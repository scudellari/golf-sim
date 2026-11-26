//! Godot 4.7 binary Variants used by courses, fixtures and records.

use crate::{Error, Real};

const WIDE: u32 = 1 << 16;
const SHARED: u32 = 1 << 31;
const MAX_DEPTH: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Integer {
    value: i64,
    wide: bool,
}

impl Integer {
    pub fn value(self) -> i64 {
        self.value
    }
}

impl From<i64> for Integer {
    fn from(value: i64) -> Self {
        Self {
            value,
            wide: i32::try_from(value).is_err(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Float {
    value: Real,
    width: FloatWidth,
}

impl Float {
    pub fn value(self) -> Real {
        self.value
    }
}

impl From<Real> for Float {
    fn from(value: Real) -> Self {
        let width = if Real::from(value as f32) == value {
            FloatWidth::Single
        } else {
            FloatWidth::Double
        };
        Self { value, width }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatWidth {
    Single,
    Double,
}

impl FloatWidth {
    fn from_header(header: u32) -> Self {
        if header & WIDE == 0 {
            Self::Single
        } else {
            Self::Double
        }
    }

    fn flag(self) -> u32 {
        match self {
            Self::Single => 0,
            Self::Double => WIDE,
        }
    }

    fn bytes(self) -> usize {
        match self {
            Self::Single => 4,
            Self::Double => 8,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Components<T> {
    pub values: T,
    pub width: FloatWidth,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ContainerType {
    #[default]
    Untyped,
    Builtin(u32),
    Class(String),
    Script(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Array {
    pub element_type: ContainerType,
    pub items: Vec<Variant>,
    pub(crate) shared: bool,
}

impl Array {
    pub fn new(items: Vec<Variant>) -> Self {
        Self::typed(ContainerType::Untyped, items)
    }

    pub fn typed(element_type: ContainerType, items: Vec<Variant>) -> Self {
        Self {
            element_type,
            items,
            shared: false,
        }
    }
}

/// Godot dictionaries serialize in insertion order, including non-string keys.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dict {
    pub key_type: ContainerType,
    pub value_type: ContainerType,
    entries: Vec<(Variant, Variant)>,
    pub(crate) shared: bool,
}

impl Dict {
    pub fn new(entries: Vec<(Variant, Variant)>) -> Self {
        Self {
            entries,
            ..Self::default()
        }
    }

    pub fn entries(&self) -> &[(Variant, Variant)] {
        &self.entries
    }

    pub fn get(&self, name: &str) -> Option<&Variant> {
        self.entries
            .iter()
            .find_map(|(key, value)| (key.as_str() == Some(name)).then_some(value))
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut Variant> {
        self.entries
            .iter_mut()
            .find_map(|(key, value)| (key.as_str() == Some(name)).then_some(value))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Variant {
    Nil,
    Bool(bool),
    Int(Integer),
    Float(Float),
    String(String),
    Vector2(Components<[Real; 2]>),
    Vector2i([i32; 2]),
    Rect2(Components<[Real; 4]>),
    Rect2i([i32; 4]),
    Vector3(Components<[Real; 3]>),
    Vector3i([i32; 3]),
    Transform2D(Components<[Real; 6]>),
    Vector4(Components<[Real; 4]>),
    Vector4i([i32; 4]),
    Plane(Components<[Real; 4]>),
    Quaternion(Components<[Real; 4]>),
    Aabb(Components<[Real; 6]>),
    Basis(Components<[Real; 9]>),
    Transform3D(Components<[Real; 12]>),
    Projection(Components<[Real; 16]>),
    Color([Real; 4]),
    StringName(String),
    Dict(Dict),
    Array(Array),
    PackedByteArray(Vec<u8>),
    PackedInt32Array(Vec<i32>),
    PackedInt64Array(Vec<i64>),
    PackedFloat32Array(Vec<Real>),
    PackedFloat64Array(Vec<Real>),
    PackedStringArray(Vec<String>),
    PackedVector2Array(Components<Vec<[Real; 2]>>),
    PackedVector3Array(Components<Vec<[Real; 3]>>),
    PackedColorArray(Vec<[Real; 4]>),
    PackedVector4Array(Components<Vec<[Real; 4]>>),
}

impl Variant {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) | Self::StringName(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&Dict> {
        match self {
            Self::Dict(dict) => Some(dict),
            _ => None,
        }
    }
}

impl From<i64> for Variant {
    fn from(value: i64) -> Self {
        Self::Int(value.into())
    }
}

impl From<Real> for Variant {
    fn from(value: Real) -> Self {
        Self::Float(value.into())
    }
}

impl From<&str> for Variant {
    fn from(value: &str) -> Self {
        Self::String(value.into())
    }
}

pub fn decode(bytes: &[u8]) -> Result<Variant, Error> {
    let mut reader = Reader { bytes, offset: 0 };
    let value = reader.value(0)?;
    if !reader.bytes.is_empty() {
        return Err(reader.error("trailing bytes"));
    }
    Ok(value)
}

pub fn encode(value: &Variant) -> Result<Vec<u8>, Error> {
    let mut writer = Writer(Vec::new());
    writer.value(value, 0)?;
    Ok(writer.0)
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn error(&self, message: impl Into<String>) -> Error {
        malformed(self.offset, message)
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], Error> {
        let (value, rest) = self
            .bytes
            .split_at_checked(len)
            .ok_or_else(|| self.error("truncated value"))?;
        self.bytes = rest;
        self.offset += len;
        Ok(value)
    }

    fn bytes<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        Ok(self.take(N)?.try_into().unwrap())
    }

    fn word(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.bytes()?))
    }

    fn int32(&mut self) -> Result<i32, Error> {
        Ok(i32::from_le_bytes(self.bytes()?))
    }

    fn int64(&mut self) -> Result<i64, Error> {
        Ok(i64::from_le_bytes(self.bytes()?))
    }

    fn padding(&mut self, len: usize) -> Result<(), Error> {
        if self.take(padding(len))?.iter().any(|&byte| byte != 0) {
            return Err(self.error("nonzero padding"));
        }
        Ok(())
    }

    fn string(&mut self, terminated: bool) -> Result<String, Error> {
        let len = self.word()? as usize;
        let mut bytes = self.take(len)?;
        self.padding(len)?;
        if terminated {
            bytes = bytes
                .strip_suffix(&[0])
                .ok_or_else(|| self.error("packed string is missing its terminator"))?;
        }
        String::from_utf8(bytes.to_vec()).map_err(|err| self.error(err.to_string()))
    }

    fn real(&mut self, width: FloatWidth) -> Result<Real, Error> {
        match width {
            FloatWidth::Single => Ok(widen(self.word()?)),
            FloatWidth::Double => Ok(Real::from_le_bytes(self.bytes()?)),
        }
    }

    fn reals<const N: usize>(&mut self, width: FloatWidth) -> Result<[Real; N], Error> {
        let mut values = [0.0; N];
        for value in &mut values {
            *value = self.real(width)?;
        }
        Ok(values)
    }

    fn ints<const N: usize>(&mut self) -> Result<[i32; N], Error> {
        let mut values = [0; N];
        for value in &mut values {
            *value = self.int32()?;
        }
        Ok(values)
    }

    fn many<T>(
        &mut self,
        count: u32,
        item_bytes: usize,
        mut read: impl FnMut(&mut Self) -> Result<T, Error>,
    ) -> Result<Vec<T>, Error> {
        let count = count as usize;
        if count > self.bytes.len() / item_bytes {
            return Err(self.error("element count exceeds remaining bytes"));
        }
        (0..count).map(|_| read(self)).collect()
    }

    fn packed<T>(
        &mut self,
        item_bytes: usize,
        read: impl FnMut(&mut Self) -> Result<T, Error>,
    ) -> Result<Vec<T>, Error> {
        let count = self.word()?;
        self.many(count, item_bytes, read)
    }

    fn components<const N: usize>(
        &mut self,
        width: FloatWidth,
    ) -> Result<Components<[Real; N]>, Error> {
        Ok(Components {
            width,
            values: self.reals(width)?,
        })
    }

    fn packed_vectors<const N: usize>(
        &mut self,
        width: FloatWidth,
    ) -> Result<Components<Vec<[Real; N]>>, Error> {
        Ok(Components {
            width,
            values: self.packed(N * width.bytes(), |reader| reader.reals(width))?,
        })
    }

    fn container_type(&mut self, kind: u32) -> Result<ContainerType, Error> {
        Ok(match kind & 3 {
            0 => ContainerType::Untyped,
            1 => ContainerType::Builtin(self.word()?),
            2 => ContainerType::Class(self.string(false)?),
            _ => ContainerType::Script(self.string(false)?),
        })
    }

    fn value(&mut self, depth: usize) -> Result<Variant, Error> {
        use Variant::*;

        let offset = self.offset;
        if depth > MAX_DEPTH {
            return Err(self.error("nesting limit exceeded"));
        }
        let header = self.word()?;
        let tag = header & 0xff;
        let flags = match tag {
            2 | 3 | 5 | 7 | 9 | 11 | 12 | 14..=19 | 35 | 36 | 38 => WIDE,
            27 => 15 << 16,
            28 => 3 << 16,
            _ => 0,
        };
        if header & !(0xff | flags) != 0 {
            return Err(malformed(offset, "unsupported header flags"));
        }
        let width = FloatWidth::from_header(header);
        Ok(match tag {
            0 => Nil,
            1 => match self.word()? {
                0 => Bool(false),
                1 => Bool(true),
                _ => return Err(malformed(offset, "invalid boolean")),
            },
            2 => {
                let wide = width == FloatWidth::Double;
                let value = if wide {
                    self.int64()?
                } else {
                    i64::from(self.int32()?)
                };
                Int(Integer { value, wide })
            }
            3 => Float(crate::variant::Float {
                value: self.real(width)?,
                width,
            }),
            4 => String(self.string(false)?),
            5 => Vector2(self.components(width)?),
            6 => Vector2i(self.ints()?),
            7 => Rect2(self.components(width)?),
            8 => Rect2i(self.ints()?),
            9 => Vector3(self.components(width)?),
            10 => Vector3i(self.ints()?),
            11 => Transform2D(self.components(width)?),
            12 => Vector4(self.components(width)?),
            13 => Vector4i(self.ints()?),
            14 => Plane(self.components(width)?),
            15 => Quaternion(self.components(width)?),
            16 => Aabb(self.components(width)?),
            17 => Basis(self.components(width)?),
            18 => Transform3D(self.components(width)?),
            19 => Projection(self.components(width)?),
            20 => Color(self.reals(FloatWidth::Single)?),
            21 => StringName(self.string(false)?),
            27 => {
                let key_type = self.container_type(header >> 16)?;
                let value_type = self.container_type(header >> 18)?;
                let count = self.word()?;
                Dict(crate::variant::Dict {
                    key_type,
                    value_type,
                    entries: self.many(count & !SHARED, 8, |reader| {
                        Ok((reader.value(depth + 1)?, reader.value(depth + 1)?))
                    })?,
                    shared: count & SHARED != 0,
                })
            }
            28 => {
                let element_type = self.container_type(header >> 16)?;
                let count = self.word()?;
                Array(crate::variant::Array {
                    element_type,
                    items: self.many(count & !SHARED, 4, |reader| reader.value(depth + 1))?,
                    shared: count & SHARED != 0,
                })
            }
            29 => {
                let count = self.word()? as usize;
                let bytes = self.take(count)?;
                self.padding(count)?;
                PackedByteArray(bytes.to_vec())
            }
            30 => PackedInt32Array(self.packed(4, Self::int32)?),
            31 => PackedInt64Array(self.packed(8, Self::int64)?),
            32 => PackedFloat32Array(self.packed(4, |reader| reader.real(FloatWidth::Single))?),
            33 => PackedFloat64Array(self.packed(8, |reader| reader.real(FloatWidth::Double))?),
            34 => PackedStringArray(self.packed(4, |reader| reader.string(true))?),
            35 => PackedVector2Array(self.packed_vectors(width)?),
            36 => PackedVector3Array(self.packed_vectors(width)?),
            37 => PackedColorArray(self.packed(16, |reader| reader.reals(FloatWidth::Single))?),
            38 => PackedVector4Array(self.packed_vectors(width)?),
            _ => return Err(malformed(offset, format!("unsupported Variant type {tag}"))),
        })
    }
}

struct Writer(Vec<u8>);

impl Writer {
    fn word(&mut self, word: u32) {
        self.0.extend(word.to_le_bytes());
    }

    fn count(&mut self, len: usize, shared: bool) -> Result<(), Error> {
        let len = u32::try_from(len)
            .ok()
            .filter(|&len| len < SHARED)
            .ok_or_else(|| malformed(self.0.len(), "value too large"))?;
        self.word(len | if shared { SHARED } else { 0 });
        Ok(())
    }

    fn padding(&mut self, len: usize) {
        self.0.resize(self.0.len() + padding(len), 0);
    }

    fn string(&mut self, value: &str, terminated: bool) -> Result<(), Error> {
        let len = value.len() + usize::from(terminated);
        self.count(len, false)?;
        self.0.extend(value.as_bytes());
        if terminated {
            self.0.push(0);
        }
        self.padding(len);
        Ok(())
    }

    fn real(&mut self, value: Real, width: FloatWidth) {
        match width {
            FloatWidth::Single => self.word(narrow(value)),
            FloatWidth::Double => self.0.extend(value.to_le_bytes()),
        }
    }

    fn reals(&mut self, values: &[Real], width: FloatWidth) {
        for &value in values {
            self.real(value, width);
        }
    }

    fn ints(&mut self, tag: u32, values: &[i32]) {
        self.word(tag);
        for value in values {
            self.0.extend(value.to_le_bytes());
        }
    }

    fn components<const N: usize>(&mut self, tag: u32, components: &Components<[Real; N]>) {
        self.word(tag | components.width.flag());
        self.reals(&components.values, components.width);
    }

    fn packed_vectors<const N: usize>(
        &mut self,
        tag: u32,
        components: &Components<Vec<[Real; N]>>,
    ) -> Result<(), Error> {
        self.word(tag | components.width.flag());
        self.count(components.values.len(), false)?;
        for value in &components.values {
            self.reals(value, components.width);
        }
        Ok(())
    }

    fn container_type(&mut self, kind: &ContainerType) -> Result<(), Error> {
        match kind {
            ContainerType::Untyped => {}
            ContainerType::Builtin(tag) => self.word(*tag),
            ContainerType::Class(name) | ContainerType::Script(name) => self.string(name, false)?,
        }
        Ok(())
    }

    fn value(&mut self, value: &Variant, depth: usize) -> Result<(), Error> {
        use Variant::*;

        if depth > MAX_DEPTH {
            return Err(malformed(self.0.len(), "nesting limit exceeded"));
        }
        match value {
            Nil => self.word(0),
            Bool(value) => {
                self.word(1);
                self.word(u32::from(*value));
            }
            Int(value) => {
                if value.wide {
                    self.word(2 | WIDE);
                    self.0.extend(value.value.to_le_bytes());
                } else {
                    self.ints(2, &[value.value as i32]);
                }
            }
            Float(value) => {
                self.word(3 | value.width.flag());
                self.real(value.value, value.width);
            }
            String(text) | StringName(text) => {
                self.word(if matches!(value, String(_)) { 4 } else { 21 });
                self.string(text, false)?;
            }
            Vector2(value) => self.components(5, value),
            Vector2i(value) => self.ints(6, value),
            Rect2(value) => self.components(7, value),
            Rect2i(value) => self.ints(8, value),
            Vector3(value) => self.components(9, value),
            Vector3i(value) => self.ints(10, value),
            Transform2D(value) => self.components(11, value),
            Vector4(value) => self.components(12, value),
            Vector4i(value) => self.ints(13, value),
            Plane(value) => self.components(14, value),
            Quaternion(value) => self.components(15, value),
            Aabb(value) => self.components(16, value),
            Basis(value) => self.components(17, value),
            Transform3D(value) => self.components(18, value),
            Projection(value) => self.components(19, value),
            Color(value) => {
                self.word(20);
                self.reals(value, FloatWidth::Single);
            }
            Dict(dict) => {
                self.word(27 | type_flag(&dict.key_type) << 16 | type_flag(&dict.value_type) << 18);
                self.container_type(&dict.key_type)?;
                self.container_type(&dict.value_type)?;
                self.count(dict.entries.len(), dict.shared)?;
                for (key, value) in &dict.entries {
                    self.value(key, depth + 1)?;
                    self.value(value, depth + 1)?;
                }
            }
            Array(array) => {
                self.word(28 | type_flag(&array.element_type) << 16);
                self.container_type(&array.element_type)?;
                self.count(array.items.len(), array.shared)?;
                for item in &array.items {
                    self.value(item, depth + 1)?;
                }
            }
            PackedByteArray(values) => {
                self.word(29);
                self.count(values.len(), false)?;
                self.0.extend(values);
                self.padding(values.len());
            }
            PackedInt32Array(values) => {
                self.word(30);
                self.count(values.len(), false)?;
                for value in values {
                    self.0.extend(value.to_le_bytes());
                }
            }
            PackedInt64Array(values) => {
                self.word(31);
                self.count(values.len(), false)?;
                for value in values {
                    self.0.extend(value.to_le_bytes());
                }
            }
            PackedFloat32Array(values) | PackedFloat64Array(values) => {
                let (tag, width) = if matches!(value, PackedFloat32Array(_)) {
                    (32, FloatWidth::Single)
                } else {
                    (33, FloatWidth::Double)
                };
                self.word(tag);
                self.count(values.len(), false)?;
                self.reals(values, width);
            }
            PackedStringArray(values) => {
                self.word(34);
                self.count(values.len(), false)?;
                for value in values {
                    self.string(value, true)?;
                }
            }
            PackedVector2Array(value) => self.packed_vectors(35, value)?,
            PackedVector3Array(value) => self.packed_vectors(36, value)?,
            PackedColorArray(values) => {
                self.word(37);
                self.count(values.len(), false)?;
                for value in values {
                    self.reals(value, FloatWidth::Single);
                }
            }
            PackedVector4Array(value) => self.packed_vectors(38, value)?,
        }
        Ok(())
    }
}

fn type_flag(kind: &ContainerType) -> u32 {
    match kind {
        ContainerType::Untyped => 0,
        ContainerType::Builtin(_) => 1,
        ContainerType::Class(_) => 2,
        ContainerType::Script(_) => 3,
    }
}

fn padding(len: usize) -> usize {
    (4 - len % 4) % 4
}

// A float cast can quiet a signaling NaN. Move the sign and payload bits without arithmetic.
fn widen(bits: u32) -> Real {
    if f32::from_bits(bits).is_nan() {
        let sign = u64::from(bits >> 31) << 63;
        let payload = u64::from(bits & 0x7f_ffff) << 29;
        Real::from_bits(sign | 0x7ff0_0000_0000_0000 | payload)
    } else {
        Real::from(f32::from_bits(bits))
    }
}

fn narrow(value: Real) -> u32 {
    if value.is_nan() {
        let bits = value.to_bits();
        let sign = ((bits >> 63) as u32) << 31;
        let payload = ((bits >> 29) as u32) & 0x7f_ffff;
        let payload = if payload == 0 { 0x40_0000 } else { payload };
        sign | 0x7f80_0000 | payload
    } else {
        (value as f32).to_bits()
    }
}

fn malformed(offset: usize, message: impl Into<String>) -> Error {
    Error::Malformed {
        path: format!("Variant byte {offset}"),
        message: message.into(),
    }
}
