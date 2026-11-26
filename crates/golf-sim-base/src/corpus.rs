//! Oracle fixtures and first differences between Variant values.

use std::{fmt, fmt::Write, fs, path::Path};

use crate::{Error, Real, variant::Variant};

const MAGIC: &[u8; 8] = b"GSCORP01";

pub fn read(path: &Path) -> Result<Variant, Error> {
    let bytes = fs::read(path).map_err(|error| Error::Io {
        file: path.display().to_string(),
        message: error.to_string(),
    })?;
    decode(&bytes)
}

pub fn decode(bytes: &[u8]) -> Result<Variant, Error> {
    let (size, body) = bytes
        .strip_prefix(MAGIC)
        .and_then(|bytes| bytes.split_at_checked(4))
        .ok_or_else(|| malformed("missing GSCORP01 header"))?;
    if body.len() != u32::from_le_bytes(size.try_into().unwrap()) as usize {
        return Err(malformed("fixture length does not match header"));
    }

    let value = crate::variant::decode(body)?;
    let dict = value.as_dict().ok_or_else(|| Error::WrongType {
        path: "$".into(),
        expected: "Dictionary",
    })?;
    let Variant::Int(format) = field(dict, "", "format")? else {
        return Err(wrong_type("format", "int"));
    };
    if format.value() != 1 {
        return Err(Error::UnknownVersion {
            what: "corpus fixture",
            version: format.value(),
        });
    }
    if !matches!(field(dict, "", "layer")?, Variant::String(_)) {
        return Err(wrong_type("layer", "String"));
    }
    let oracle = field(dict, "", "oracle")?
        .as_dict()
        .ok_or_else(|| wrong_type("oracle", "Dictionary"))?;
    if !matches!(field(oracle, "oracle.", "versions")?, Variant::Dict(_)) {
        return Err(wrong_type("oracle.versions", "Dictionary"));
    }
    for field_name in ["commit", "hash"] {
        if !matches!(field(oracle, "oracle.", field_name)?, Variant::String(_)) {
            return Err(wrong_type(format!("oracle.{field_name}"), "String"));
        }
    }
    let Variant::Array(cases) = field(dict, "", "cases")? else {
        return Err(wrong_type("cases", "Array"));
    };
    for (index, case) in cases.items.iter().enumerate() {
        let case = case
            .as_dict()
            .ok_or_else(|| wrong_type(format!("cases[{index}]"), "Dictionary"))?;
        match case.entries().first() {
            Some((Variant::String(key), Variant::String(_))) if key == "id" => {}
            Some((Variant::String(key), _)) if key == "id" => {
                return Err(wrong_type(format!("cases[{index}].id"), "String"));
            }
            _ => {
                return Err(Error::Malformed {
                    path: format!("cases[{index}].id"),
                    message: "expected id as the first field".into(),
                });
            }
        }
    }
    Ok(value)
}

fn field<'a>(
    dict: &'a crate::variant::Dict,
    prefix: &str,
    name: &str,
) -> Result<&'a Variant, Error> {
    dict.get(name).ok_or_else(|| Error::MissingKey {
        path: format!("{prefix}{name}"),
    })
}

fn wrong_type(path: impl Into<String>, expected: &'static str) -> Error {
    Error::WrongType {
        path: path.into(),
        expected,
    }
}

fn malformed(message: &str) -> Error {
    Error::Malformed {
        path: "GSCORP01".into(),
        message: message.into(),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tolerance {
    pub absolute: Real,
    pub relative: Real,
}

pub fn first_difference(expected: &Variant, actual: &Variant) -> Option<String> {
    first_difference_with(expected, actual, |_| None)
}

/// The hook selects tolerances for individual numeric values. Either bound may match.
/// The relative bound uses the larger absolute magnitude of the two numbers.
pub fn first_difference_with(
    expected: &Variant,
    actual: &Variant,
    tolerance: impl Fn(&str) -> Option<Tolerance>,
) -> Option<String> {
    let mut comparer = Comparer {
        path: String::new(),
        tolerance: &tolerance,
    };
    if comparer.same(expected, actual) {
        return None;
    }
    if comparer.path.is_empty() {
        comparer.path.push('$');
    }
    Some(comparer.path)
}

struct Comparer<'a> {
    path: String,
    tolerance: &'a dyn Fn(&str) -> Option<Tolerance>,
}

impl Comparer<'_> {
    fn at(&mut self, segment: fmt::Arguments<'_>, same: impl FnOnce(&mut Self) -> bool) -> bool {
        let end = self.path.len();
        self.path.write_fmt(segment).unwrap();
        if same(self) {
            self.path.truncate(end);
            true
        } else {
            false
        }
    }

    fn at_key(
        &mut self,
        key: &Variant,
        index: usize,
        same: impl FnOnce(&mut Self) -> bool,
    ) -> bool {
        match key.as_str() {
            Some(name) if identifier(name) && self.path.is_empty() => {
                self.at(format_args!("{name}"), same)
            }
            Some(name) if identifier(name) => self.at(format_args!(".{name}"), same),
            Some(name) => self.at(format_args!("[{name:?}]"), same),
            None => self.at(format_args!("[key {index}]"), same),
        }
    }

    fn tolerance(&self) -> Option<Tolerance> {
        (self.tolerance)(if self.path.is_empty() {
            "$"
        } else {
            &self.path
        })
    }

    fn reals(&self, a: Real, b: Real) -> bool {
        if a.to_bits() == b.to_bits() {
            return true;
        }
        let Some(tolerance) = self.tolerance() else {
            return false;
        };
        if !a.is_finite() || !b.is_finite() {
            return false;
        }
        let delta = (a - b).abs();
        let scale = a.abs().max(b.abs());
        let relative = if delta.is_finite() {
            delta / scale
        } else {
            (a / scale - b / scale).abs()
        };
        delta <= tolerance.absolute || relative <= tolerance.relative
    }

    fn integers(&self, a: i64, b: i64) -> bool {
        a == b
            || self.tolerance().is_some_and(|tolerance| {
                let delta = a.abs_diff(b) as Real;
                let scale = a.unsigned_abs().max(b.unsigned_abs()) as Real;
                delta <= tolerance.absolute || delta / scale <= tolerance.relative
            })
    }

    fn sequence<T>(
        &mut self,
        a: &[T],
        b: &[T],
        mut same: impl FnMut(&mut Self, &T, &T) -> bool,
    ) -> bool {
        for (index, (a, b)) in a.iter().zip(b).enumerate() {
            if !self.at(format_args!("[{index}]"), |this| same(this, a, b)) {
                return false;
            }
        }
        a.len() == b.len() || self.at(format_args!("[{}]", a.len().min(b.len())), |_| false)
    }

    fn axes<T>(
        &mut self,
        a: &[T],
        b: &[T],
        names: &[&str],
        same: impl Fn(&Self, &T, &T) -> bool,
    ) -> bool {
        for ((a, b), name) in a.iter().zip(b).zip(names) {
            let dot = if self.path.is_empty() { "" } else { "." };
            if !self.at(format_args!("{dot}{name}"), |this| same(this, a, b)) {
                return false;
            }
        }
        true
    }

    fn real_axes(&mut self, a: &[Real], b: &[Real], names: &[&str]) -> bool {
        self.axes(a, b, names, |this, a, b| this.reals(*a, *b))
    }

    fn int_axes(&mut self, a: &[i32], b: &[i32], names: &[&str]) -> bool {
        self.axes(a, b, names, |this, a, b| {
            this.integers(i64::from(*a), i64::from(*b))
        })
    }

    fn same(&mut self, a: &Variant, b: &Variant) -> bool {
        use Variant::*;
        const XY: &[&str] = &["x", "y"];
        const XYZ: &[&str] = &["x", "y", "z"];
        const XYZW: &[&str] = &["x", "y", "z", "w"];
        const RGBA: &[&str] = &["r", "g", "b", "a"];

        match (a, b) {
            (Int(a), Int(b)) => self.integers(a.value(), b.value()),
            (Float(a), Float(b)) => self.reals(a.value(), b.value()),
            (Vector2(a), Vector2(b)) => self.real_axes(&a.values, &b.values, XY),
            (Vector3(a), Vector3(b)) => self.real_axes(&a.values, &b.values, XYZ),
            (Vector4(a), Vector4(b)) | (Quaternion(a), Quaternion(b)) | (Plane(a), Plane(b)) => {
                self.real_axes(&a.values, &b.values, XYZW)
            }
            (Vector2i(a), Vector2i(b)) => self.int_axes(a, b, XY),
            (Vector3i(a), Vector3i(b)) => self.int_axes(a, b, XYZ),
            (Vector4i(a), Vector4i(b)) => self.int_axes(a, b, XYZW),
            (Rect2i(a), Rect2i(b)) => self.sequence(a, b, |this, a, b| {
                this.integers(i64::from(*a), i64::from(*b))
            }),
            (Rect2(a), Rect2(b)) => self.real_sequence(&a.values, &b.values),
            (Transform2D(a), Transform2D(b)) | (Aabb(a), Aabb(b)) => {
                self.real_sequence(&a.values, &b.values)
            }
            (Basis(a), Basis(b)) => self.real_sequence(&a.values, &b.values),
            (Transform3D(a), Transform3D(b)) => self.real_sequence(&a.values, &b.values),
            (Projection(a), Projection(b)) => self.real_sequence(&a.values, &b.values),
            (Color(a), Color(b)) => self.real_axes(a, b, RGBA),
            (PackedVector2Array(a), PackedVector2Array(b)) => {
                self.sequence(&a.values, &b.values, |this, a, b| this.real_axes(a, b, XY))
            }
            (PackedVector3Array(a), PackedVector3Array(b)) => {
                self.sequence(&a.values, &b.values, |this, a, b| this.real_axes(a, b, XYZ))
            }
            (PackedVector4Array(a), PackedVector4Array(b)) => {
                self.sequence(&a.values, &b.values, |this, a, b| {
                    this.real_axes(a, b, XYZW)
                })
            }
            (PackedColorArray(a), PackedColorArray(b)) => {
                self.sequence(a, b, |this, a, b| this.real_axes(a, b, RGBA))
            }
            (PackedFloat32Array(a), PackedFloat32Array(b))
            | (PackedFloat64Array(a), PackedFloat64Array(b)) => self.real_sequence(a, b),
            (PackedInt32Array(a), PackedInt32Array(b)) => self.sequence(a, b, |this, a, b| {
                this.integers(i64::from(*a), i64::from(*b))
            }),
            (PackedInt64Array(a), PackedInt64Array(b)) => {
                self.sequence(a, b, |this, a, b| this.integers(*a, *b))
            }
            (PackedByteArray(a), PackedByteArray(b)) => self.sequence(a, b, |_, a, b| a == b),
            (PackedStringArray(a), PackedStringArray(b)) => self.sequence(a, b, |_, a, b| a == b),
            (Array(a), Array(b)) => {
                a.element_type == b.element_type && self.sequence(&a.items, &b.items, Self::same)
            }
            (Dict(a), Dict(b)) => {
                a.key_type == b.key_type
                    && a.value_type == b.value_type
                    && self.entries(a.entries(), b.entries())
            }
            _ => a == b,
        }
    }

    fn real_sequence(&mut self, a: &[Real], b: &[Real]) -> bool {
        self.sequence(a, b, |this, a, b| this.reals(*a, *b))
    }

    fn entries(&mut self, a: &[(Variant, Variant)], b: &[(Variant, Variant)]) -> bool {
        for (index, ((a_key, a_value), (b_key, b_value))) in a.iter().zip(b).enumerate() {
            if !self.at_key(a_key, index, |this| {
                first_difference(a_key, b_key).is_none() && this.same(a_value, b_value)
            }) {
                return false;
            }
        }
        let index = a.len().min(b.len());
        match a.get(index).or_else(|| b.get(index)) {
            Some((key, _)) => self.at_key(key, index, |_| false),
            None => true,
        }
    }
}

fn identifier(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte == b'_' || byte.is_ascii_alphabetic())
        && bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
}
