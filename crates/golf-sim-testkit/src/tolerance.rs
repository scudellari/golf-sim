//! Tolerances for comparisons with recorded results.

use std::{
    borrow::Cow,
    cell::OnceCell,
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
    error::Error as StdError,
    fmt, fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use golf_sim_base::{
    Error, Real,
    corpus::{self, Tolerance},
    digest::{object, sha256_hex},
    variant::{self, Dict, Variant},
};
use serde::Deserialize;

type CorpusResult<T> = Result<T, Box<dyn StdError>>;

const TOLERANCES: &str = "tolerances.toml";
const DEPARTURES: &str = "departures.toml";
const SINGLE_PRECISION_STEP: Real = f32::EPSILON as Real;

fn read_config<T: serde::de::DeserializeOwned>(path: &Path, default: &str) -> CorpusResult<T> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => Cow::Owned(contents),
        Err(error) if error.kind() == ErrorKind::NotFound => Cow::Borrowed(default),
        Err(error) => return Err(format!("{}: {error}", path.display()).into()),
    };
    toml::from_str(&contents).map_err(|error| format!("{}: {error}", path.display()).into())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Bounds {
    #[serde(default)]
    abs: Real,
    #[serde(default)]
    rel: Real,
    f32_steps: Option<Real>,
    #[serde(default)]
    scale_from: Vec<String>,
}

impl Bounds {
    fn valid(&self) -> bool {
        [self.abs, self.rel, self.f32_steps.unwrap_or_default()]
            .into_iter()
            .all(|bound| bound.is_finite() && bound >= 0.0)
    }

    fn tolerance(&self, step_bound: Option<Real>) -> Option<Tolerance> {
        (self.abs > 0.0 || self.rel > 0.0 || step_bound.is_some()).then_some(Tolerance {
            absolute: self.abs.max(step_bound.unwrap_or_default()),
            relative: self.rel,
        })
    }

    fn input_scale(&self, scales: &BTreeMap<&str, Real>) -> Real {
        self.scale_from
            .iter()
            .map(|path| scales[path.as_str()])
            .fold(1.0, Real::max)
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Layer {
    abs: Real,
    rel: Real,
    discrete_share: Real,
    scalar_ulps: Option<u64>,
    f32_steps: Option<Real>,
    #[serde(default)]
    scale_from: Vec<String>,
    #[serde(default)]
    fields: BTreeMap<String, Bounds>,
}

impl Layer {
    fn load(path: &Path, key: &str, layer: &str) -> CorpusResult<Option<Self>> {
        let mut layers: BTreeMap<String, Self> =
            read_config(path, include_str!("../tolerances.toml"))?;
        let name = if layers.contains_key(key) { key } else { layer };
        let Some(layer) = layers.remove(name) else {
            return Ok(None);
        };
        if !layer.default_bounds().valid()
            || !layer.fields.values().all(|bounds| bounds.valid())
            || !(0.0..=1.0).contains(&layer.discrete_share)
        {
            return Err(format!("{}: invalid bounds for layer {name:?}", path.display()).into());
        }
        Ok(Some(layer))
    }

    pub(crate) fn for_fixture(testdata: &Path, key: &str) -> CorpusResult<Self> {
        let layer = key.split_once('/').map_or(key, |(layer, _)| layer);
        Self::load(&testdata.join(TOLERANCES), key, layer)?
            .ok_or_else(|| format!("no tolerances for {key:?}").into())
    }

    pub(crate) fn checkpoint_discrete_difference(
        &self,
        expected: &Variant,
        actual: &Variant,
    ) -> Option<String> {
        let mut numbers = Numbers {
            exact_integers: true,
            ..Numbers::default()
        };
        let expected = split_numbers(expected, "$", &mut numbers);
        numbers.values.clear();
        let actual = split_numbers(actual, "$", &mut numbers);
        corpus::first_difference(&expected, &actual)
    }

    pub(crate) fn checkpoint_difference(
        &self,
        expected: &Variant,
        actual: &Variant,
        prefix: &str,
    ) -> Option<String> {
        let default = self.default_bounds();
        let values = OnceCell::new();
        corpus::first_difference_with(expected, actual, |path| {
            let full = if path == "$" {
                prefix.to_owned()
            } else if prefix.is_empty() {
                path.to_owned()
            } else {
                numeric_path(prefix, path)
            };
            let field = self.bounds_for(&full);
            let row = field.unwrap_or(&default);
            // The stroke checkpoint rows have no external scale sources.
            assert!(row.scale_from.is_empty());
            let measurement = if self.scalar_ulps.is_some() || row.f32_steps.is_some() {
                let fields = values.get_or_init(|| OutputFields::new(expected, actual));
                fields
                    .numeric(|_| 1.0)
                    .find(|(key, ..)| *key == path)
                    .map(|(_, a, b, delta)| (a, b, delta))
            } else {
                None
            };
            self.numeric_tolerance(field, row, measurement)
        })
    }

    fn default_bounds(&self) -> Bounds {
        Bounds {
            abs: self.abs,
            rel: self.rel,
            f32_steps: self.f32_steps,
            scale_from: self.scale_from.clone(),
        }
    }

    fn bounds_for(&self, path: &str) -> Option<&Bounds> {
        self.fields
            .iter()
            .filter(|(prefix, _)| matches_field_path(path, prefix))
            .max_by_key(|(prefix, _)| (prefix.len(), Reverse(prefix.matches("[*]").count())))
            .map(|(_, bounds)| bounds)
    }

    fn numeric_tolerance(
        &self,
        field: Option<&Bounds>,
        bounds: &Bounds,
        measurement: Option<(Number, Number, Delta)>,
    ) -> Option<Tolerance> {
        let scalar_ulps = self.scalar_ulps.filter(|_| field.is_none());
        let mut step_bound = None;
        if let Some((expected, actual, delta)) = measurement {
            if let Some(limit) = scalar_ulps
                && matches!(expected, Number::Scalar(_))
            {
                if expected.within_ulps(actual, limit) {
                    return Some(Tolerance {
                        absolute: delta.absolute,
                        relative: 0.0,
                    });
                }
                // Preserve the legacy scalar ULP rule for rows without a step bound.
                bounds.f32_steps?;
            }
            step_bound = bounds
                .f32_steps
                .filter(|limit| delta.steps <= *limit)
                .map(|_| delta.absolute);
        }
        bounds.tolerance(step_bound)
    }
}

fn matches_field_path(mut path: &str, mut pattern: &str) -> bool {
    while let Some((literal, tail)) = pattern.split_once("[*]") {
        let Some((index, rest)) = path
            .strip_prefix(literal)
            .and_then(|path| path.strip_prefix('['))
            .and_then(|path| path.split_once(']'))
        else {
            return false;
        };
        if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
            return false;
        }
        path = rest;
        pattern = tail;
    }
    at_or_below(path, pattern)
}

fn at_or_below(path: &str, prefix: &str) -> bool {
    path.strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(['.', '[']))
}

pub struct Fixture {
    fields: Dict,
    testdata: PathBuf,
}

impl Fixture {
    pub fn read(path: &Path) -> Result<Self, Error> {
        let Variant::Dict(fixture) = corpus::read(path)? else {
            unreachable!("validated corpus dictionary");
        };
        let location = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let testdata = location
            .ancestors()
            .skip(1)
            .find(|parent| parent.file_name().is_some_and(|name| name == "testdata"))
            .unwrap_or_else(|| Path::new("testdata"))
            .to_path_buf();
        Ok(Self::from_header(fixture, testdata))
    }

    pub(crate) fn from_header(fields: Dict, testdata: PathBuf) -> Self {
        Self { fields, testdata }
    }

    pub fn layer(&self) -> &str {
        let key = self.key();
        key.split_once('/').map_or(key, |(layer, _)| layer)
    }

    fn key(&self) -> &str {
        self.fields
            .get("layer")
            .and_then(Variant::as_str)
            .expect("validated corpus layer")
    }

    fn require_layer(&self, layer: &str) -> CorpusResult<()> {
        if self.layer() != layer {
            return Err(format!("fixture layer is {:?}, requested {layer:?}", self.layer()).into());
        }
        Ok(())
    }

    fn cases(&self) -> impl Iterator<Item = Case<'_>> {
        let Some(Variant::Array(cases)) = self.fields.get("cases") else {
            unreachable!("validated corpus cases");
        };
        let layer = self.layer();
        cases
            .items
            .iter()
            .enumerate()
            .map(move |(index, case)| Case {
                index,
                layer,
                fields: case.as_dict().expect("validated corpus case"),
            })
    }
}

struct Case<'a> {
    index: usize,
    fields: &'a Dict,
    layer: &'a str,
}

impl Case<'_> {
    fn id(&self) -> &str {
        self.fields
            .get("id")
            .and_then(Variant::as_str)
            .expect("validated corpus case id")
    }

    fn expected(&self) -> Result<Cow<'_, Variant>, Error> {
        if let Some(output) = self.fields.get("output") {
            return Ok(Cow::Borrowed(output));
        }
        if self.layer == "variant"
            && self.fields.get("op").and_then(Variant::as_str) == Some("snapshot")
        {
            // The digest fixture predates the output envelope and stores its outputs beside id.
            let fields = ["value", "bytes", "digest"]
                .into_iter()
                .map(|key| {
                    self.fields
                        .get(key)
                        .map(|value| (key, value.clone()))
                        .ok_or_else(|| Error::MissingKey {
                            path: format!("cases[{}].output.{key}", self.index),
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(Cow::Owned(object(fields)));
        }
        Err(Error::MissingKey {
            path: format!("cases[{}].output", self.index),
        })
    }

    fn output_path(&self, field: &str) -> String {
        let prefix = format!("cases[{}].output", self.index);
        match field.as_bytes().first() {
            Some(b'$') if field == "$" => prefix,
            Some(b'[') => format!("{prefix}{field}"),
            _ => format!("{prefix}.{field}"),
        }
    }

    fn call(&self) -> Option<&str> {
        self.fields.get("call").and_then(Variant::as_str)
    }

    fn run_error(&self, error: impl fmt::Display) -> String {
        if let Some(call) = self.call() {
            format!(
                "cases[{}] (case {:?}, call {call:?}): {error}",
                self.index,
                self.id()
            )
        } else {
            format!("cases[{}] (case {:?}): {error}", self.index, self.id())
        }
    }

    fn difference(&self, field: &str, delta: Option<Delta>) -> Difference {
        Difference {
            path: self.output_path(field),
            case_id: self.id().into(),
            call: self.call().map(str::to_owned),
            delta,
        }
    }

    fn source_scales<'a, 'b>(
        &self,
        layer: &'a Layer,
        default: &'a Bounds,
        output: impl FnOnce() -> &'b OutputFields,
    ) -> CorpusResult<BTreeMap<&'a str, Real>> {
        if default.scale_from.is_empty()
            && layer.fields.values().all(|row| row.scale_from.is_empty())
        {
            return Ok(BTreeMap::new());
        }
        let sources: BTreeSet<_> = output()
            .expected
            .iter()
            .flat_map(|(path, _)| &layer.bounds_for(path).unwrap_or(default).scale_from)
            .map(String::as_str)
            .collect();
        if sources.is_empty() {
            return Ok(BTreeMap::new());
        }
        let mut numbers = Numbers {
            paths: Some(BTreeSet::new()),
            ..Numbers::default()
        };
        numbers.record_path("$");
        split_dict(self.fields, "$", &mut numbers);
        sources
            .into_iter()
            .map(|source| {
                numbers
                    .magnitude_at(source)
                    .map(|scale| (source, scale))
                    .ok_or_else(|| {
                        format!(
                            "cases[{}] (case {:?}): scale_from path {source:?} is missing",
                            self.index,
                            self.id()
                        )
                        .into()
                    })
            })
            .collect()
    }
}

/// A fixture runner with no captured state.
pub type Runner = fn(&Dict) -> Result<Variant, Error>;

/// Run a recorded Variant snapshot through the codec and compute its digest.
pub fn variant_snapshot(case: &Dict) -> Result<Variant, Error> {
    let case = case.get("input").and_then(Variant::as_dict).unwrap_or(case);
    let bytes = case.get("bytes").ok_or_else(|| Error::MissingKey {
        path: "bytes".into(),
    })?;
    let Variant::PackedByteArray(bytes) = bytes else {
        return Err(Error::WrongType {
            path: "bytes".into(),
            expected: "PackedByteArray",
        });
    };
    let value = variant::decode(bytes)?;
    let bytes = variant::encode(&value)?;
    let digest = sha256_hex(&bytes);
    Ok(object([
        ("value", value),
        ("bytes", Variant::PackedByteArray(bytes)),
        ("digest", Variant::String(digest)),
    ]))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Delta {
    pub absolute: Real,
    pub relative: Real,
    pub steps: Real,
}

impl fmt::Display for Delta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "abs {:e}, rel {:e}, steps {:e}",
            self.absolute, self.relative, self.steps
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Difference {
    pub path: String,
    pub case_id: String,
    pub call: Option<String>,
    pub delta: Option<Delta>,
}

impl fmt::Display for Difference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (case {:?}", self.path, self.case_id)?;
        if let Some(call) = &self.call {
            write!(f, ", call {call:?}")?;
        }
        f.write_str("): ")?;
        match self.delta {
            Some(delta) => write!(f, "{delta}"),
            None => f.write_str("discrete difference"),
        }
    }
}

impl std::error::Error for Difference {}

#[derive(Clone, Copy)]
enum Number {
    Integer(i64),
    Component { value: Real, scale: Real },
    Scalar(Real),
}

impl Number {
    fn component(value: Real) -> Self {
        Self::Component {
            value,
            scale: finite_magnitude(value),
        }
    }

    fn magnitude(self) -> Real {
        match self {
            Self::Integer(value) => value.unsigned_abs() as Real,
            Self::Component { value, .. } | Self::Scalar(value) => finite_magnitude(value),
        }
    }

    fn delta(self, actual: Self, input_scale: Real) -> Option<Delta> {
        let expected_scale = match self {
            Self::Component { scale, .. } => scale,
            _ => self.magnitude(),
        };
        let step_size = SINGLE_PRECISION_STEP * input_scale.max(expected_scale);
        match (self, actual) {
            (Self::Integer(a), Self::Integer(b)) => {
                let absolute = a.abs_diff(b) as Real;
                let scale = a.unsigned_abs().max(b.unsigned_abs()) as Real;
                let relative = if absolute == 0.0 {
                    0.0
                } else {
                    absolute / scale
                };
                Some(Delta {
                    absolute,
                    relative,
                    steps: absolute / step_size,
                })
            }
            (Self::Component { value: a, .. }, Self::Component { value: b, .. })
            | (Self::Scalar(a), Self::Scalar(b)) => {
                if a.to_bits() == b.to_bits() {
                    return Some(Delta {
                        absolute: 0.0,
                        relative: 0.0,
                        steps: 0.0,
                    });
                }
                if !a.is_finite() || !b.is_finite() {
                    return Some(Delta {
                        absolute: Real::INFINITY,
                        relative: Real::INFINITY,
                        steps: Real::INFINITY,
                    });
                }
                let absolute = (a - b).abs();
                let scale = a.abs().max(b.abs());
                let relative = if absolute == 0.0 {
                    0.0
                } else if absolute.is_finite() {
                    absolute / scale
                } else {
                    (a / scale - b / scale).abs()
                };
                let steps = if absolute.is_finite() {
                    absolute / step_size
                } else {
                    (a / step_size - b / step_size).abs()
                };
                Some(Delta {
                    absolute,
                    relative,
                    steps,
                })
            }
            _ => None,
        }
    }

    fn variant(self) -> Variant {
        match self {
            Self::Integer(value) => value.into(),
            Self::Component { value, .. } | Self::Scalar(value) => value.into(),
        }
    }

    fn within_ulps(self, other: Self, limit: u64) -> bool {
        let (Self::Scalar(a), Self::Scalar(b)) = (self, other) else {
            return false;
        };
        if !a.is_finite() || !b.is_finite() {
            return false;
        }
        let a_magnitude = a.abs().to_bits();
        let b_magnitude = b.abs().to_bits();
        let distance = if a.is_sign_negative() == b.is_sign_negative() {
            a_magnitude.abs_diff(b_magnitude)
        } else {
            a_magnitude + b_magnitude
        };
        distance <= limit
    }
}

fn finite_magnitude(value: Real) -> Real {
    if value.is_finite() { value.abs() } else { 0.0 }
}

#[derive(Default)]
struct Numbers {
    values: Vec<(String, Number)>,
    paths: Option<BTreeSet<String>>,
    exact_integers: bool,
}

impl Numbers {
    fn record_path(&mut self, path: &str) {
        if let Some(paths) = &mut self.paths {
            paths.insert(path.into());
        }
    }

    fn push(&mut self, path: String, number: Number) {
        self.values.push((path, number));
    }

    fn magnitude_at(&self, source: &str) -> Option<Real> {
        self.values
            .iter()
            .filter(|(path, _)| source == "$" || !source.is_empty() && at_or_below(path, source))
            .map(|(_, number)| number.magnitude())
            .reduce(Real::max)
            .or_else(|| self.paths.as_ref()?.contains(source).then_some(0.0))
    }
}

fn numeric_path(parent: &str, field: &str) -> String {
    if parent == "$" {
        field.into()
    } else {
        format!("{parent}.{field}")
    }
}

fn indexed_path(parent: &str, index: usize) -> String {
    format!("{}[{index}]", if parent == "$" { "" } else { parent })
}

fn key_path(parent: &str, key: &Variant, index: usize) -> String {
    let prefix = if parent == "$" { "" } else { parent };
    let Some(key) = key.as_str() else {
        return format!("{prefix}[key {index}]");
    };
    let mut bytes = key.bytes();
    if bytes
        .next()
        .is_some_and(|byte| byte == b'_' || byte.is_ascii_alphabetic())
        && bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
    {
        numeric_path(parent, key)
    } else {
        format!("{prefix}[{key:?}]")
    }
}

fn component_path(parent: &str, axes: &[&str], index: usize) -> String {
    axes.get(index).map_or_else(
        || indexed_path(parent, index),
        |axis| numeric_path(parent, axis),
    )
}

fn components<T: Copy + Default>(
    values: &mut [T],
    axes: &[&str],
    path: &str,
    numbers: &mut Numbers,
    number: impl Fn(T) -> Number,
) {
    for (index, value) in values.iter_mut().enumerate() {
        numbers.push(
            component_path(path, axes, index),
            number(std::mem::take(value)),
        );
    }
}

fn packed_components<const N: usize>(
    values: &mut [[Real; N]],
    axes: &[&str],
    path: &str,
    numbers: &mut Numbers,
) {
    for (index, value) in values.iter_mut().enumerate() {
        real_components(value, N, axes, &indexed_path(path, index), numbers);
    }
}

fn real_components(
    values: &mut [Real],
    width: usize,
    axes: &[&str],
    path: &str,
    numbers: &mut Numbers,
) {
    for (group, values) in values.chunks_exact_mut(width).enumerate() {
        let scale = values
            .iter()
            .copied()
            .map(finite_magnitude)
            .fold(0.0, Real::max);
        for (index, value) in values.iter_mut().enumerate() {
            numbers.push(
                component_path(path, axes, group * width + index),
                Number::Component {
                    value: std::mem::take(value),
                    scale,
                },
            );
        }
    }
}

fn split_dict(dict: &Dict, path: &str, numbers: &mut Numbers) -> Dict {
    let entries = dict
        .entries()
        .iter()
        .enumerate()
        .map(|(index, (key, value))| {
            (
                key.clone(),
                split_numbers(value, &key_path(path, key, index), numbers),
            )
        })
        .collect();
    let mut projected = Dict::new(entries);
    projected.key_type.clone_from(&dict.key_type);
    projected.value_type.clone_from(&dict.value_type);
    projected
}

// Keep the original shape with zeroes in numeric slots so the base comparer can report
// discrete changes even when an earlier numeric value is non-finite.
fn split_numbers(value: &Variant, path: &str, numbers: &mut Numbers) -> Variant {
    use Variant::*;
    const XY: &[&str] = &["x", "y"];
    const XYZ: &[&str] = &["x", "y", "z"];
    const XYZW: &[&str] = &["x", "y", "z", "w"];
    const RGBA: &[&str] = &["r", "g", "b", "a"];
    numbers.record_path(path);
    let real = Number::component;
    let int = |value| Number::Integer(i64::from(value));
    let mut discrete = match value {
        Array(array) => {
            let items = array
                .items
                .iter()
                .enumerate()
                .map(|(index, value)| split_numbers(value, &indexed_path(path, index), numbers))
                .collect();
            return Array(golf_sim_base::variant::Array::typed(
                array.element_type.clone(),
                items,
            ));
        }
        Dict(dict) => return Dict(split_dict(dict, path, numbers)),
        value => value.clone(),
    };
    match &mut discrete {
        Int(value) if !numbers.exact_integers => {
            numbers.push(path.into(), Number::Integer(value.value()));
            *value = 0.into();
        }
        Float(value) => {
            numbers.push(path.into(), Number::Scalar(value.value()));
            *value = 0.0.into();
        }
        Vector2(v) => real_components(&mut v.values, 2, XY, path, numbers),
        Vector3(v) => real_components(&mut v.values, 3, XYZ, path, numbers),
        Vector4(v) => real_components(&mut v.values, 4, XYZW, path, numbers),
        Quaternion(v) | Plane(v) => components(&mut v.values, XYZW, path, numbers, real),
        Vector2i(v) => components(v, XY, path, numbers, int),
        Vector3i(v) => components(v, XYZ, path, numbers, int),
        Vector4i(v) => components(v, XYZW, path, numbers, int),
        Rect2i(v) => components(v, &[], path, numbers, int),
        Rect2(v) => components(&mut v.values, &[], path, numbers, real),
        Transform2D(v) => real_components(&mut v.values, 2, &[], path, numbers),
        Aabb(v) => components(&mut v.values, &[], path, numbers, real),
        Basis(v) => real_components(&mut v.values, 3, &[], path, numbers),
        Transform3D(v) => real_components(&mut v.values, 3, &[], path, numbers),
        Projection(v) => components(&mut v.values, &[], path, numbers, real),
        Color(v) => components(v, RGBA, path, numbers, real),
        PackedVector2Array(v) => packed_components(&mut v.values, XY, path, numbers),
        PackedVector3Array(v) => packed_components(&mut v.values, XYZ, path, numbers),
        PackedVector4Array(v) => packed_components(&mut v.values, XYZW, path, numbers),
        PackedColorArray(v) => {
            for (index, color) in v.iter_mut().enumerate() {
                components(color, RGBA, &indexed_path(path, index), numbers, real);
            }
        }
        PackedFloat32Array(v) | PackedFloat64Array(v) => components(v, &[], path, numbers, real),
        PackedInt32Array(v) => components(v, &[], path, numbers, int),
        PackedInt64Array(v) => components(v, &[], path, numbers, Number::Integer),
        _ => {}
    }
    discrete
}

struct OutputFields {
    expected: Vec<(String, Number)>,
    actual: BTreeMap<String, Number>,
    discrete: Option<String>,
}

impl OutputFields {
    fn new(expected: &Variant, actual: &Variant) -> Self {
        let mut expected_numbers = Numbers::default();
        let mut actual_numbers = Numbers::default();
        let expected_discrete = split_numbers(expected, "$", &mut expected_numbers);
        let actual_discrete = split_numbers(actual, "$", &mut actual_numbers);
        Self {
            expected: expected_numbers.values,
            actual: actual_numbers.values.into_iter().collect(),
            discrete: corpus::first_difference(&expected_discrete, &actual_discrete),
        }
    }

    fn numeric(
        &self,
        input_scale: impl Fn(&str) -> Real,
    ) -> impl Iterator<Item = (&str, Number, Number, Delta)> {
        self.expected.iter().filter_map(move |(field, expected)| {
            let actual = *self.actual.get(field)?;
            let delta = expected.delta(actual, input_scale(field))?;
            Some((field.as_str(), *expected, actual, delta))
        })
    }
}

fn compare_case(
    case: &Case<'_>,
    expected: &Variant,
    actual: &Variant,
    layer: &Layer,
    allow_discrete: bool,
) -> CorpusResult<(Option<Difference>, bool)> {
    let values = OnceCell::new();
    let values = || values.get_or_init(|| OutputFields::new(expected, actual));
    let default = layer.default_bounds();
    let scales = case.source_scales(layer, &default, values)?;
    let input_scale = |path: &str| {
        layer
            .bounds_for(path)
            .unwrap_or(&default)
            .input_scale(&scales)
    };
    let measurements = OnceCell::new();
    let measurements = || {
        measurements.get_or_init(|| {
            values()
                .numeric(input_scale)
                .map(|(field, expected, actual, delta)| (field, (expected, actual, delta)))
                .collect::<BTreeMap<_, _>>()
        })
    };
    let tolerance = |path: &str| {
        let field = layer.bounds_for(path);
        let bounds = field.unwrap_or(&default);
        let measurement = (layer.scalar_ulps.is_some() || bounds.f32_steps.is_some())
            .then(|| measurements().get(path).copied())
            .flatten();
        layer.numeric_tolerance(field, bounds, measurement)
    };
    let Some(first) = corpus::first_difference_with(expected, actual, tolerance) else {
        return Ok((None, false));
    };
    let fields = values();
    if fields.discrete.as_deref() != Some(&first) {
        let delta = measurements()
            .get(first.as_str())
            .map(|&(_, _, delta)| delta);
        return Ok((Some(case.difference(&first, delta)), true));
    }
    let difference = case.difference(&first, None);
    if !allow_discrete {
        return Ok((Some(difference), true));
    }
    if !matches!(case.layer, "hole" | "round") {
        for (path, expected, actual, delta) in fields.numeric(input_scale) {
            if corpus::first_difference_with(&expected.variant(), &actual.variant(), |_| {
                tolerance(path)
            })
            .is_some()
            {
                return Ok((Some(case.difference(path, Some(delta))), true));
            }
        }
    }
    Ok((Some(difference), false))
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Departure {
    pub id: String,
    pub layer: String,
    pub change: String,
}

impl fmt::Display for Departure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "skipped case {:?} ({}; change {})",
            self.id, self.layer, self.change
        )
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Departures {
    #[serde(default)]
    departure: Vec<Departure>,
    #[serde(default)]
    pub field_departure: Vec<FieldDepartureRule>,
    #[serde(default)]
    pub precision_departure: Vec<PrecisionDepartureRule>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PrecisionDepartureRule {
    pub fixture: String,
    pub call: String,
    pub ids_sha256: BTreeSet<String>,
    pub threshold: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FieldDepartureRule {
    pub call: String,
    pub fields: Vec<String>,
    pub version: String,
    pub change: String,
}

impl Departures {
    pub(crate) fn load(testdata: &Path) -> CorpusResult<Self> {
        read_config(
            &testdata.join(DEPARTURES),
            include_str!("../departures.toml"),
        )
    }

    pub(crate) fn cases_for(&self, key: &str) -> BTreeMap<&str, &Departure> {
        let layer = key.split_once('/').map_or(key, |(layer, _)| layer);
        let mut cases = BTreeMap::new();
        for departure in &self.departure {
            if departure.layer == key || departure.layer == layer {
                cases.entry(departure.id.as_str()).or_insert(departure);
            }
        }
        cases
    }
}

impl Fixture {
    fn selected_cases(&self) -> CorpusResult<(Vec<Case<'_>>, Vec<Departure>)> {
        let departures = Departures::load(&self.testdata)?;
        let by_case = departures.cases_for(self.key());
        let mut selected = Vec::new();
        let mut skipped = Vec::new();
        for case in self.cases() {
            match by_case.get(case.id()) {
                Some(departure) => skipped.push((*departure).clone()),
                None => selected.push(case),
            }
        }
        Ok((selected, skipped))
    }
}

pub(crate) enum CaseOutcome {
    Compared {
        actual: Variant,
        departures: Vec<FieldDeparture>,
    },
    ModelDeparture {
        actual: Variant,
        first: crate::corpus::physics::models::AffectedEvent,
        change: &'static str,
    },
    PrecisionDeparture {
        actual: Variant,
        change: String,
    },
    Refused(String),
    ModifiedCourse {
        course: String,
        revision: String,
    },
}

impl From<Variant> for CaseOutcome {
    fn from(actual: Variant) -> Self {
        Self::Compared {
            actual,
            departures: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CaseRefusal {
    pub id: String,
    pub reason: String,
    modified_course: Option<String>,
}

impl CaseRefusal {
    fn other(id: &str, reason: String) -> Self {
        Self {
            id: id.into(),
            reason,
            modified_course: None,
        }
    }

    fn modified(id: &str, course: String, revision: String) -> Self {
        Self {
            id: id.into(),
            reason: format!("modified copy of {course}"),
            modified_course: Some(revision),
        }
    }
}

fn write_refusals(f: &mut fmt::Formatter<'_>, refused: &[CaseRefusal]) -> fmt::Result {
    let mut revisions = BTreeMap::new();
    for refusal in refused {
        writeln!(f, "{refusal}")?;
        if let Some(revision) = &refusal.modified_course {
            *revisions.entry((&refusal.reason, revision)).or_insert(0) += 1;
        }
    }
    for ((reason, revision), count) in revisions {
        writeln!(f, "{reason}, revision {revision:?}: {count} cases")?;
    }
    Ok(())
}

impl fmt::Display for CaseRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "refused case {:?}: {}", self.id, self.reason)?;
        if let Some(revision) = &self.modified_course {
            write!(f, " (revision {revision:?})")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldDeparture {
    pub id: String,
    pub field: String,
    pub change: String,
}

impl fmt::Display for FieldDeparture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "departed field {:?} (case {:?}; change {})",
            self.field, self.id, self.change
        )
    }
}

#[derive(Debug, Default)]
pub struct CheckReport {
    pub checked: usize,
    pub discrete: Vec<Difference>,
    pub skipped: Vec<Departure>,
    pub refused: Vec<CaseRefusal>,
    pub field_departures: Vec<FieldDeparture>,
    failures: Vec<String>,
}

impl fmt::Display for CheckReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for departure in &self.skipped {
            writeln!(f, "{departure}")?;
        }
        write_refusals(f, &self.refused)?;
        if !self.refused.is_empty() {
            writeln!(
                f,
                "refused {} of {} cases",
                self.refused.len(),
                self.case_count()
            )?;
        }
        for departure in &self.field_departures {
            writeln!(f, "{departure}")?;
        }
        for difference in &self.discrete {
            writeln!(f, "{difference} (within discrete allowance)")?;
        }
        if !self.failures.is_empty() {
            for failure in &self.failures {
                writeln!(f, "{failure}")?;
            }
            writeln!(f, "failed {} cases", self.failures.len())?;
        }
        let noun = if self.checked == 1 { "case" } else { "cases" };
        writeln!(f, "checked {} {noun}", self.checked)
    }
}

impl CheckReport {
    fn case_count(&self) -> usize {
        self.checked + self.refused.len() + self.skipped.len()
    }
}

#[derive(Debug)]
struct CheckFailure(CheckReport);

impl fmt::Display for CheckFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)?;
        if exceeds_refusal_limit(&self.0.refused, self.0.case_count()) {
            writeln!(f, "refusals exceed 1% of fixture cases")?;
        }
        Ok(())
    }
}

impl StdError for CheckFailure {}

fn exceeds_refusal_limit(refused: &[CaseRefusal], total: usize) -> bool {
    let counted = refused
        .iter()
        .filter(|entry| entry.modified_course.is_none())
        .count();
    counted > total / 100
}

/// Run a fixture against its configured tolerances and skip intentional departures.
///
/// A row for the full fixture key takes precedence over the layer row.
/// Use the fixture's enclosing `testdata` directory or `testdata` in the working directory.
/// Built-in defaults let the installed command run with standalone fixtures.
pub fn check<E: fmt::Display>(
    path: &Path,
    layer: &str,
    run: impl FnMut(&Dict) -> Result<Variant, E>,
) -> CorpusResult<CheckReport> {
    Fixture::read(path)?.check(layer, run)
}

impl Fixture {
    pub(crate) fn check<E: fmt::Display>(
        &self,
        layer: &str,
        mut run: impl FnMut(&Dict) -> Result<Variant, E>,
    ) -> CorpusResult<CheckReport> {
        self.check_cases(layer, |case| run(case).map(Into::into))
            .map_err(|error| match error.downcast::<CheckFailure>() {
                Ok(failure) => {
                    let report = failure.0;
                    let mut text = String::new();
                    use fmt::Write;
                    for departure in &report.skipped {
                        writeln!(text, "{departure}").unwrap();
                    }
                    text.push_str(&report.failures[0]);
                    text.into()
                }
                Err(error) => error,
            })
    }

    pub(crate) fn check_cases<E: fmt::Display>(
        &self,
        layer: &str,
        mut run: impl FnMut(&Dict) -> Result<CaseOutcome, E>,
    ) -> CorpusResult<CheckReport> {
        self.require_layer(layer)?;
        let config = self.testdata.join(TOLERANCES);
        let limits = Layer::load(&config, self.key(), layer)?
            .ok_or_else(|| format!("{}: no tolerances for layer {layer:?}", config.display()))?;
        let (cases, skipped) = self.selected_cases()?;
        let mut report = CheckReport {
            skipped,
            ..CheckReport::default()
        };
        let mut discrete = Vec::new();
        for case in cases {
            let outcome = match run(case.fields) {
                Ok(outcome) => outcome,
                Err(error) => {
                    report.checked += 1;
                    report.failures.push(case.run_error(error));
                    continue;
                }
            };
            let (actual, departures) = match outcome {
                CaseOutcome::Compared { actual, departures } => (actual, departures),
                CaseOutcome::PrecisionDeparture { change, .. } => {
                    report.skipped.push(Departure {
                        id: case.id().into(),
                        layer: layer.into(),
                        change,
                    });
                    continue;
                }
                CaseOutcome::ModelDeparture {
                    actual,
                    first,
                    change,
                } => {
                    let expected = case.expected()?;
                    if let Some((expected_prefix, actual_prefix)) =
                        first.unaffected(&expected).zip(first.unaffected(&actual))
                    {
                        let (difference, _) =
                            compare_case(&case, &expected_prefix, &actual_prefix, &limits, false)?;
                        if let Some(difference) = difference {
                            report.checked += 1;
                            report.failures.push(difference.to_string());
                        } else {
                            report.skipped.push(Departure {
                                id: case.id().into(),
                                layer: layer.into(),
                                change: change.into(),
                            });
                        }
                        continue;
                    }
                    (actual, Vec::new())
                }
                CaseOutcome::Refused(reason) => {
                    report.refused.push(CaseRefusal::other(case.id(), reason));
                    continue;
                }
                CaseOutcome::ModifiedCourse { course, revision } => {
                    report
                        .refused
                        .push(CaseRefusal::modified(case.id(), course, revision));
                    continue;
                }
            };
            report.checked += 1;
            report.field_departures.extend(departures);
            let expected = case.expected()?;
            let (difference, failure) = compare_case(
                &case,
                &expected,
                &actual,
                &limits,
                limits.discrete_share > 0.0,
            )?;
            if let Some(difference) = difference {
                if failure {
                    report.failures.push(difference.to_string());
                } else {
                    discrete.push(difference);
                }
            }
        }
        for difference in discrete {
            if (report.discrete.len() + 1) as Real / report.checked as Real <= limits.discrete_share
            {
                report.discrete.push(difference);
            } else {
                report.failures.push(difference.to_string());
            }
        }
        if !report.failures.is_empty()
            || exceeds_refusal_limit(&report.refused, report.case_count())
        {
            return Err(CheckFailure(report).into());
        }
        Ok(report)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Maximum {
    pub size: Real,
    pub case_id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldDrift {
    pub absolute: Maximum,
    pub relative: Maximum,
    pub steps: Maximum,
}

#[derive(Debug, Default, PartialEq)]
pub struct DriftReport {
    pub numeric: BTreeMap<String, FieldDrift>,
    pub discrete: Vec<Difference>,
    pub skipped: Vec<Departure>,
    pub refused: Vec<CaseRefusal>,
    pub field_departures: Vec<FieldDeparture>,
    failures: Vec<String>,
    #[cfg(test)]
    pub(crate) out_of_bounds: Vec<Difference>,
}

impl fmt::Display for DriftReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for departure in &self.skipped {
            writeln!(f, "{departure}")?;
        }
        write_refusals(f, &self.refused)?;
        for departure in &self.field_departures {
            writeln!(f, "{departure}")?;
        }
        for failure in &self.failures {
            writeln!(f, "{failure}")?;
        }
        for (field, drift) in &self.numeric {
            writeln!(
                f,
                "{field}: max abs {:e} (case {:?}), max rel {:e} (case {:?}), max steps {:e} (case {:?})",
                drift.absolute.size,
                drift.absolute.case_id,
                drift.relative.size,
                drift.relative.case_id,
                drift.steps.size,
                drift.steps.case_id
            )?;
        }
        for difference in &self.discrete {
            writeln!(f, "{difference}")?;
        }
        Ok(())
    }
}

impl Fixture {
    /// Measure all comparable fields and list discrete differences, in fixture order by case.
    pub fn drift<E: fmt::Display>(
        &self,
        layer: &str,
        mut run: impl FnMut(&Dict) -> Result<Variant, E>,
    ) -> CorpusResult<DriftReport> {
        self.drift_cases(layer, |case| run(case).map(Into::into))
    }

    pub(crate) fn drift_cases<E: fmt::Display>(
        &self,
        layer: &str,
        mut run: impl FnMut(&Dict) -> Result<CaseOutcome, E>,
    ) -> CorpusResult<DriftReport> {
        self.require_layer(layer)?;
        let limits =
            Layer::load(&self.testdata.join(TOLERANCES), self.key(), layer)?.unwrap_or_default();
        let default = limits.default_bounds();
        let (cases, skipped) = self.selected_cases()?;
        let total = cases.len() + skipped.len();
        let mut report = DriftReport {
            skipped,
            ..DriftReport::default()
        };
        for case in cases {
            let expected = case.expected()?;
            let actual = match run(case.fields) {
                Ok(CaseOutcome::Compared { actual, departures }) => {
                    report.field_departures.extend(departures);
                    actual
                }
                Ok(CaseOutcome::PrecisionDeparture { actual, change }) => {
                    report.skipped.push(Departure {
                        id: case.id().into(),
                        layer: layer.into(),
                        change,
                    });
                    actual
                }
                Ok(CaseOutcome::ModelDeparture {
                    actual,
                    first,
                    change,
                }) => {
                    if let Some((reference, prefix)) =
                        first.unaffected(&expected).zip(first.unaffected(&actual))
                        && compare_case(&case, &reference, &prefix, &limits, false)?
                            .0
                            .is_none()
                    {
                        report.skipped.push(Departure {
                            id: case.id().into(),
                            layer: layer.into(),
                            change: change.into(),
                        });
                    }
                    actual
                }
                Ok(CaseOutcome::Refused(reason)) => {
                    report.refused.push(CaseRefusal::other(case.id(), reason));
                    continue;
                }
                Ok(CaseOutcome::ModifiedCourse { course, revision }) => {
                    report
                        .refused
                        .push(CaseRefusal::modified(case.id(), course, revision));
                    continue;
                }
                Err(error) => {
                    report.failures.push(case.run_error(error));
                    continue;
                }
            };
            let fields = OutputFields::new(&expected, &actual);
            let scales = case.source_scales(&limits, &default, || &fields)?;
            let input_scale = |path: &str| {
                limits
                    .bounds_for(path)
                    .unwrap_or(&default)
                    .input_scale(&scales)
            };
            for (field, expected, actual, delta) in fields.numeric(input_scale) {
                #[cfg(not(test))]
                let _ = (expected, actual);
                #[cfg(test)]
                {
                    let row = limits.bounds_for(field);
                    let tolerance = limits.numeric_tolerance(
                        row,
                        row.unwrap_or(&default),
                        Some((expected, actual, delta)),
                    );
                    if corpus::first_difference_with(&expected.variant(), &actual.variant(), |_| {
                        tolerance
                    })
                    .is_some()
                    {
                        report
                            .out_of_bounds
                            .push(case.difference(field, Some(delta)));
                    }
                }
                let maximum = |size| Maximum {
                    size,
                    case_id: case.id().into(),
                };
                let drift = report
                    .numeric
                    .entry(field.into())
                    .or_insert_with(|| FieldDrift {
                        absolute: maximum(delta.absolute),
                        relative: maximum(delta.relative),
                        steps: maximum(delta.steps),
                    });
                if delta.absolute > drift.absolute.size {
                    drift.absolute = maximum(delta.absolute);
                }
                if delta.relative > drift.relative.size {
                    drift.relative = maximum(delta.relative);
                }
                if delta.steps > drift.steps.size {
                    drift.steps = maximum(delta.steps);
                }
            }
            if let Some(field) = fields.discrete {
                report.discrete.push(case.difference(&field, None));
            }
        }
        if exceeds_refusal_limit(&report.refused, total) {
            return Err(format!("{report}refusals exceed 1% of fixture cases").into());
        }
        Ok(report)
    }
}

#[cfg(test)]
mod refusal_tests {
    use super::*;
    use golf_sim_base::variant::Array;

    #[test]
    fn refusals_are_listed_and_do_not_stop_later_comparisons() {
        for (total, passes_cap) in [(100, true), (99, false)] {
            let rows = (0..total)
                .map(|i| {
                    object([
                        ("id", Variant::String(i.to_string())),
                        ("output", Variant::Bool(true)),
                    ])
                })
                .collect();
            let header = object([
                ("layer", "physics/flight".into()),
                ("cases", Variant::Array(Array::new(rows))),
            ]);
            let Variant::Dict(header) = header else {
                unreachable!()
            };
            let fixture = Fixture::from_header(header, PathBuf::from("testdata"));
            let mut ran = 0;
            let check = fixture.check_cases("physics", |_| {
                ran += 1;
                Ok::<_, std::convert::Infallible>(if ran == 1 {
                    CaseOutcome::Refused("unsupported air sampler".into())
                } else if ran <= 4 {
                    CaseOutcome::ModifiedCourse {
                        course: "synthetic".into(),
                        revision: if ran == 4 { "another" } else { "revision" }.into(),
                    }
                } else {
                    Variant::Bool(true).into()
                })
            });
            assert_eq!(ran, total);
            assert_eq!(check.is_ok(), passes_cap);
            let printed = match check {
                Ok(report) => report.to_string(),
                Err(error) => error.to_string(),
            };
            assert!(printed.contains("refused case \"0\": unsupported air sampler"));
            assert!(printed.contains("modified copy of synthetic, revision \"revision\": 2 cases"));
            assert!(printed.contains("modified copy of synthetic, revision \"another\": 1 cases"));
            assert_eq!(printed.contains("refusals exceed 1%"), !passes_cap);
        }
    }
}
