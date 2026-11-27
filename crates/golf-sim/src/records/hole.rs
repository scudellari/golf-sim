//! The format 5 hole record and its replay compatibility checks.

use std::fmt;

use golf_sim_base::{
    ClubId, Error, GOLF_SIM_COMMIT, Real, Surface, Vec2, Vec3,
    variant::{self, Array, Components, Dict, FloatWidth, Variant},
};
use golf_sim_conditions::lies::{BallLie, DivotLie, Sit};
use golf_sim_course::course::Course;
use golf_sim_stroke::address::{Address, Setting};
use golf_sim_swing::{execution::Execution, hands::Swing};

use crate::setup::{HoleSetup, PlaySettings, dictionary, vector3};

const MAGIC: &[u8; 8] = b"LLHOLE01";
const FORMAT: &str = "links-lab-hole-record";
pub const FORMAT_VERSION: &str = "5";

const BUILD_VERSIONS: [(&str, &str); 5] = [
    ("release", env!("CARGO_PKG_VERSION")),
    ("commit", GOLF_SIM_COMMIT),
    ("physics", golf_sim_physics::tunables::VERSION),
    ("tuning", golf_sim_stroke::tuning::VERSION),
    // The rules crate does not expose a model version yet.
    ("rules", "unversioned"),
];

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Refusal {
    WrongMagic,
    LengthMismatch,
    WrongFormat,
    UnsupportedFormat(String),
    IncompatibleVersion {
        model: &'static str,
        recorded: String,
        current: &'static str,
    },
    CourseRevision {
        recorded: String,
        supplied: String,
    },
    Invalid(Error),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongMagic => f.write_str("expected LLHOLE01"),
            Self::LengthMismatch => f.write_str("hole record length does not match its header"),
            Self::WrongFormat => f.write_str("expected a hole record dictionary"),
            Self::UnsupportedFormat(version) => {
                write!(f, "unsupported hole record version {version}")
            }
            Self::IncompatibleVersion {
                model,
                recorded,
                current,
            } => {
                write!(
                    f,
                    "the record needs {model} version {recorded}, and this build has {current}"
                )
            }
            Self::CourseRevision { recorded, supplied } => {
                write!(
                    f,
                    "the record needs course revision {recorded}, and this course is {supplied}"
                )
            }
            Self::Invalid(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for Refusal {}

impl From<Error> for Refusal {
    fn from(error: Error) -> Self {
        Self::Invalid(error)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HoleRecord {
    pub versions: Dict,
    pub setup: HoleSetup,
    pub driver: Dict,
    pub entries: Vec<Entry>,
}

impl HoleRecord {
    pub fn begin(setup: HoleSetup, driver: Dict) -> Self {
        let versions = dictionary(
            [("record", FORMAT_VERSION.into())]
                .into_iter()
                .chain(BUILD_VERSIONS.map(|(key, value)| (key, value.into()))),
        );
        Self {
            versions,
            setup,
            driver,
            entries: Vec::new(),
        }
    }

    pub fn console_driver() -> Dict {
        dictionary([("kind", "console".into())])
    }

    pub fn agent_driver(tool: &str) -> Dict {
        dictionary([("kind", "agent".into()), ("tool", tool.into())])
    }

    pub fn check_course(&self, course: &Course) -> Result<(), Refusal> {
        if self.setup.course_revision != course.revision() {
            return Err(Refusal::CourseRevision {
                recorded: self.setup.course_revision.clone(),
                supplied: course.revision().into(),
            });
        }
        if self.setup.course_id != course.data().id {
            return Err(malformed("setup.course_id", "the record names a different course").into());
        }
        if self.setup.tee(course).is_none() {
            return Err(
                malformed("setup.tee_id", "the course has no such tee on this hole").into(),
            );
        }
        if self.setup.cup(course).is_none() {
            return Err(
                malformed("setup.cup_id", "the course has no such cup on this hole").into(),
            );
        }
        Ok(())
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, Refusal> {
        let is_agent = is_agent(&self.driver)?;
        let entries = self
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let dict = match entry {
                    Entry::Stroke(stroke) => stroke.to_dict(index, is_agent)?,
                    Entry::Decision(decision) => decision.to_dict(),
                };
                Ok(Variant::Dict(dict))
            })
            .collect::<Result<_, Error>>()?;
        let dict = dictionary([
            ("format", FORMAT.into()),
            ("versions", Variant::Dict(self.versions.clone())),
            (
                "course_revision",
                self.setup.course_revision.as_str().into(),
            ),
            ("setup", Variant::Dict(self.setup.to_dict())),
            ("settings", Variant::Dict(self.setup.settings.to_dict())),
            ("driver", Variant::Dict(self.driver.clone())),
            ("entries", Variant::Array(Array::new(entries))),
        ]);
        let body = variant::encode(&Variant::Dict(dict))?;
        let size = u32::try_from(body.len()).map_err(|_| Refusal::LengthMismatch)?;
        let mut bytes = Vec::with_capacity(MAGIC.len() + 4 + body.len());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&size.to_le_bytes());
        bytes.extend_from_slice(&body);
        Ok(bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Refusal> {
        let dict = decode_envelope(bytes)?;
        let fields = Fields::new(&dict, "");
        let versions = fields.nested("versions")?;
        let written = versions.string("record")?;
        if written != FORMAT_VERSION {
            return Err(Refusal::UnsupportedFormat(written.into()));
        }
        for (model, current) in BUILD_VERSIONS {
            let recorded = versions.string(model)?;
            if recorded != current {
                return Err(Refusal::IncompatibleVersion {
                    model,
                    recorded: recorded.into(),
                    current,
                });
            }
        }
        let setup = fields.nested("setup")?.wide();
        setup.vector3("wind_mps")?;
        let setup = HoleSetup::from_dict(
            setup.dict,
            fields.string("course_revision")?.into(),
            PlaySettings::from_dict(fields.nested("settings")?.dict),
        )?;
        let driver = fields.nested("driver")?.dict;
        let agent = is_agent(driver)?;
        let entries = fields
            .array("entries")?
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let entry = Fields::new(
                    entry
                        .as_dict()
                        .ok_or_else(|| wrong_type(format!("entries[{index}]"), "Dictionary"))?,
                    format!("entries[{index}]"),
                )
                .wide();
                let kind = entry.string("kind")?;
                if kind == "stroke" {
                    Ok(Entry::Stroke(Box::new(StrokeEntry::read(entry, agent)?)))
                } else {
                    let kind = DecisionKind::from_name(kind)
                        .ok_or_else(|| malformed(&entry.key("kind"), "unknown decision"))?;
                    Ok(Entry::Decision(DecisionEntry::read(kind, entry)?))
                }
            })
            .collect::<Result<_, Error>>()?;
        Ok(Self {
            versions: versions.dict.clone(),
            setup,
            driver: driver.clone(),
            entries,
        })
    }
}

/// Decode the shared framing. Callers check the version before reading the payload fields.
pub fn decode_envelope(bytes: &[u8]) -> Result<Dict, Refusal> {
    let rest = bytes.strip_prefix(MAGIC).ok_or(Refusal::WrongMagic)?;
    let (size, body) = rest
        .split_first_chunk::<4>()
        .ok_or(Refusal::LengthMismatch)?;
    if body.len() != u32::from_le_bytes(*size) as usize {
        return Err(Refusal::LengthMismatch);
    }
    match variant::decode(body)? {
        Variant::Dict(dict) if dict.get("format") == Some(&Variant::from(FORMAT)) => Ok(dict),
        _ => Err(Refusal::WrongFormat),
    }
}

fn is_agent(driver: &Dict) -> Result<bool, Error> {
    let fields = Fields::new(driver, "driver");
    match fields.string("kind")? {
        "console" => Ok(false),
        "agent" => {
            fields.string("tool")?;
            Ok(true)
        }
        _ => Err(malformed("driver.kind", "unknown driver")),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Stroke(Box<StrokeEntry>),
    Decision(DecisionEntry),
}

#[derive(Clone, Debug, PartialEq)]
pub struct StrokeEntry {
    pub facts: StrokeFacts,
    pub swing: Dict,
    pub address_usec: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SwingPress {
    Press(i64),
    Steer { usec: i64, delta_m: Vec2 },
}

impl StrokeEntry {
    pub fn pressed_swing(presses: impl IntoIterator<Item = SwingPress>) -> Dict {
        let presses = presses.into_iter().map(|input| match input {
            SwingPress::Press(usec) => usec.into(),
            SwingPress::Steer { usec, delta_m } => Variant::Dict(dictionary([
                ("usec", usec.into()),
                ("delta_m", vector2(delta_m)),
            ])),
        });
        dictionary([
            ("mode", "pressed".into()),
            ("presses", Variant::Array(Array::new(presses.collect()))),
        ])
    }

    pub fn swing_dict(swing: Swing) -> Dict {
        match swing {
            Swing::Exact {
                power,
                timing_fault,
                strike_m,
            } => dictionary([
                ("mode", "exact".into()),
                ("power", power.into()),
                ("timing_fault", timing_fault.into()),
                ("strike_m", strike_m.map_or(Variant::Nil, vector2)),
            ]),
            Swing::Skill(profile) => {
                let values = dictionary([
                    ("sigma_p_ms", profile.sigma_p_ms.into()),
                    ("bias_t_ms", profile.bias_t_ms.into()),
                    ("sigma_t_ms", profile.sigma_t_ms.into()),
                    ("sigma_s_mm", profile.sigma_s_mm.into()),
                    ("tau_ms", profile.tau_ms.into()),
                ]);
                dictionary([
                    ("mode", "skill".into()),
                    (
                        "profile",
                        Variant::Dict(dictionary([
                            ("name", profile.name.into()),
                            ("values", Variant::Dict(values)),
                        ])),
                    ),
                ])
            }
        }
    }

    fn read(fields: Fields<'_>, agent: bool) -> Result<Self, Error> {
        let swing = fields.nested("swing")?;
        check_swing(&swing)?;
        let address_usec = fields
            .dict
            .get("address_usec")
            .map(|_| fields.integer("address_usec"))
            .transpose()?;
        check_address_time(address_usec, agent, &fields.key("address_usec"))?;
        Ok(Self {
            facts: StrokeFacts::read(&fields)?,
            swing: swing.dict.clone(),
            address_usec,
        })
    }

    fn to_dict(&self, index: usize, agent: bool) -> Result<Dict, Error> {
        let path = format!("entries[{index}]");
        let mut swing = self.swing.clone();
        if let Some(Variant::Vector2(strike)) = swing.get_mut("strike_m")
            && self.swing.get("mode").and_then(Variant::as_str) == Some("exact")
        {
            strike.width = FloatWidth::Double;
        }
        check_swing(&Fields::new(&swing, format!("{path}.swing")).wide())?;
        check_address_time(self.address_usec, agent, &format!("{path}.address_usec"))?;
        let mut fields = self.facts.fields();
        fields.push(("swing", Variant::Dict(swing)));
        if let Some(time) = self.address_usec {
            fields.push(("address_usec", time.into()));
        }
        Ok(dictionary(fields))
    }
}

fn check_address_time(time: Option<i64>, agent: bool, path: &str) -> Result<(), Error> {
    match time {
        Some(time) if time < 0 => Err(malformed(path, "expected nonnegative microseconds")),
        None if agent => Err(Error::MissingKey { path: path.into() }),
        _ => Ok(()),
    }
}

fn check_swing(swing: &Fields<'_>) -> Result<(), Error> {
    match swing.string("mode")? {
        "pressed" => {
            for (index, value) in swing.array("presses")?.iter().enumerate() {
                if matches!(value, Variant::Int(_)) {
                    continue;
                }
                let path = format!("{}[{index}]", swing.key("presses"));
                let steer = value
                    .as_dict()
                    .ok_or_else(|| wrong_type(path.clone(), "stamped press or steer"))?;
                let steer = Fields::new(steer, path).wide();
                steer.integer("usec")?;
                steer.vector2("delta_m")?;
            }
        }
        "exact" => {
            swing.real("power")?;
            swing.real("timing_fault")?;
            if !matches!(swing.get("strike_m")?, Variant::Nil) {
                swing.vector2("strike_m")?;
            }
        }
        "skill" => {
            let profile = swing.nested("profile")?;
            profile.string("name")?;
            let values = profile.nested("values")?;
            for key in [
                "sigma_p_ms",
                "bias_t_ms",
                "sigma_t_ms",
                "sigma_s_mm",
                "tau_ms",
            ] {
                values.real(key)?;
            }
        }
        _ => return Err(malformed(&swing.key("mode"), "unknown swing mode")),
    }
    Ok(())
}

/// The resolved address and judgment, followed by the result that the next entry reads.
#[derive(Clone, Debug, PartialEq)]
pub struct StrokeFacts {
    pub attempt: i64,
    pub club_id: ClubId,
    pub address: Address,
    pub slot: i64,
    pub shape: i64,
    pub height: i64,
    pub aim: Vec3,
    pub flagstick_in: bool,
    pub execution: Execution,
    pub gust_time_s: Real,
    pub clock_s: Real,
    pub rest: Vec3,
    pub lie: BallLie,
}

impl StrokeFacts {
    pub fn from_dict(dict: &Dict, index: usize) -> Result<Self, Error> {
        Self::read(&Fields::new(dict, format!("entries[{index}]")))
    }

    fn read(fields: &Fields<'_>) -> Result<Self, Error> {
        let address = fields.nested("address")?;
        for setting in Setting::ALL {
            address.real(setting.key())?;
        }
        Ok(Self {
            attempt: fields.integer("attempt")?,
            club_id: ClubId::from_name(fields.string("club_id")?)
                .ok_or_else(|| malformed(&fields.key("club_id"), "unknown club"))?,
            address: Address::from_dict(address.dict),
            slot: fields.integer("slot")?,
            shape: fields.integer("shape")?,
            height: fields.integer("height")?,
            aim: fields.vector3("aim")?,
            flagstick_in: fields.boolean("flagstick_in")?,
            execution: read_execution(fields.nested("execution")?)?,
            gust_time_s: fields.real("gust_time_s")?,
            clock_s: fields.real("clock_s")?,
            rest: fields.vector3("rest")?,
            lie: read_lie(fields.nested("lie")?)?,
        })
    }

    fn fields(&self) -> Vec<(&str, Variant)> {
        vec![
            ("kind", "stroke".into()),
            ("attempt", self.attempt.into()),
            ("club_id", self.club_id.name().into()),
            ("address", Variant::Dict(self.address.to_dict())),
            ("slot", self.slot.into()),
            ("shape", self.shape.into()),
            ("height", self.height.into()),
            ("aim", vector3(self.aim)),
            ("flagstick_in", Variant::Bool(self.flagstick_in)),
            ("execution", Variant::Dict(execution_dict(self.execution))),
            ("gust_time_s", self.gust_time_s.into()),
            ("clock_s", self.clock_s.into()),
            ("rest", vector3(self.rest)),
            ("lie", Variant::Dict(lie_dict(self.lie))),
        ]
    }
}

fn execution_dict(execution: Execution) -> Dict {
    dictionary([
        ("power", execution.power.into()),
        ("timing_error_s", execution.timing_error_s.into()),
        ("timing_fault", execution.timing_fault.into()),
        ("timed_out", Variant::Bool(execution.timed_out)),
        ("has_strike", Variant::Bool(execution.strike_m.is_some())),
        (
            "strike_m",
            vector2(execution.strike_m.unwrap_or(Vec2::ZERO)),
        ),
        ("impact_usec", execution.impact_usec.into()),
        ("backswing_s", execution.backswing_s.into()),
        ("ring_s", execution.ring_s.into()),
    ])
}

fn read_execution(fields: Fields<'_>) -> Result<Execution, Error> {
    let strike = fields.vector2("strike_m")?;
    Ok(Execution {
        power: fields.real("power")?,
        timing_error_s: fields.real("timing_error_s")?,
        timing_fault: fields.real("timing_fault")?,
        timed_out: fields.boolean("timed_out")?,
        strike_m: fields.boolean("has_strike")?.then_some(strike),
        impact_usec: fields.integer("impact_usec")?,
        backswing_s: fields.real("backswing_s")?,
        ring_s: fields.real("ring_s")?,
    })
}

fn lie_dict(lie: BallLie) -> Dict {
    dictionary([
        ("spot", vector3(lie.spot)),
        ("surface", (lie.surface as i64).into()),
        ("sit", (lie.sit as i64).into()),
        ("ball_height_m", lie.ball_height_m.into()),
        ("grass_height_m", lie.grass_height_m.into()),
        ("grass_density", lie.grass_density.into()),
        ("sand_firmness", lie.sand_firmness.into()),
        ("divot", (lie.divot as i64).into()),
    ])
}

fn read_lie(fields: Fields<'_>) -> Result<BallLie, Error> {
    Ok(BallLie {
        spot: fields.vector3("spot")?,
        surface: fields.choice("surface", Surface::ALL)?,
        sit: fields.choice("sit", &[Sit::Up, Sit::Normal, Sit::Down])?,
        ball_height_m: fields.real("ball_height_m")?,
        grass_height_m: fields.real("grass_height_m")?,
        grass_density: fields.real("grass_density")?,
        sand_firmness: fields.real("sand_firmness")?,
        divot: fields.choice(
            "divot",
            &[
                DivotLie::None,
                DivotLie::Fresh,
                DivotLie::Old,
                DivotLie::SandFilled,
            ],
        )?,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecisionKind {
    GoForward,
    Relief,
    Drop,
    Place,
}

impl DecisionKind {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "go_forward" => Some(Self::GoForward),
            "relief" => Some(Self::Relief),
            "drop" => Some(Self::Drop),
            "place" => Some(Self::Place),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::GoForward => "go_forward",
            Self::Relief => "relief",
            Self::Drop => "drop",
            Self::Place => "place",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DecisionEntry {
    pub kind: DecisionKind,
    pub option: i64,
    pub point: Vec3,
    pub footprint: Vec<Vec2>,
    pub ruling: Dict,
}

impl DecisionEntry {
    pub fn from_dict(kind: DecisionKind, dict: &Dict, index: usize) -> Result<Self, Error> {
        Self::read(kind, Fields::new(dict, format!("entries[{index}]")))
    }

    fn read(kind: DecisionKind, fields: Fields<'_>) -> Result<Self, Error> {
        let Variant::PackedVector2Array(footprint) = fields.get("footprint")? else {
            return Err(wrong_type(fields.key("footprint"), "PackedVector2Array"));
        };
        fields.check_width("footprint", footprint.width)?;
        if !matches!(footprint.values.len(), 0 | 3)
            || footprint
                .values
                .iter()
                .any(|&[x, y]| !x.is_finite() || !y.is_finite() || Vec2::new(x, y).length() > 2.0)
        {
            return Err(malformed(
                &fields.key("footprint"),
                "invalid stance footprint",
            ));
        }
        let ruling = fields.nested("ruling")?.dict;
        if ruling.entries().is_empty() {
            return Err(malformed(&fields.key("ruling"), "empty ruling"));
        }
        let option = fields.integer("option")?;
        if option < 0 {
            return Err(malformed(&fields.key("option"), "negative relief option"));
        }
        Ok(Self {
            kind,
            option,
            point: fields.vector3("point")?,
            footprint: footprint
                .values
                .iter()
                .map(|&[x, y]| Vec2::new(x, y))
                .collect(),
            ruling: ruling.clone(),
        })
    }

    fn to_dict(&self) -> Dict {
        dictionary([
            ("kind", self.kind.name().into()),
            ("option", self.option.into()),
            ("point", vector3(self.point)),
            (
                "footprint",
                Variant::PackedVector2Array(Components {
                    values: self
                        .footprint
                        .iter()
                        .map(|point| [point.x, point.y])
                        .collect(),
                    width: FloatWidth::Double,
                }),
            ),
            ("ruling", Variant::Dict(wide_ruling(&self.ruling))),
        ])
    }
}

fn wide_ruling(dict: &Dict) -> Dict {
    Dict::new(
        dict.entries()
            .iter()
            .map(|(key, value)| (key.clone(), wide_positions(value)))
            .collect(),
    )
}

fn wide_positions(value: &Variant) -> Variant {
    let mut value = value.clone();
    match &mut value {
        Variant::Vector2(point) => point.width = FloatWidth::Double,
        Variant::Vector3(point) => point.width = FloatWidth::Double,
        Variant::PackedVector2Array(points) => points.width = FloatWidth::Double,
        Variant::PackedVector3Array(points) => points.width = FloatWidth::Double,
        Variant::Dict(dict) => *dict = wide_ruling(dict),
        Variant::Array(array) => array.items = array.items.iter().map(wide_positions).collect(),
        _ => {}
    }
    value
}

pub(crate) struct Fields<'a> {
    dict: &'a Dict,
    path: String,
    wide_vectors: bool,
}

impl<'a> Fields<'a> {
    pub(crate) fn new(dict: &'a Dict, path: impl Into<String>) -> Self {
        Self {
            dict,
            path: path.into(),
            wide_vectors: false,
        }
    }

    fn wide(mut self) -> Self {
        self.wide_vectors = true;
        self
    }

    fn key(&self, key: &str) -> String {
        if self.path.is_empty() {
            key.into()
        } else {
            format!("{}.{key}", self.path)
        }
    }

    fn get(&self, key: &str) -> Result<&'a Variant, Error> {
        self.dict.get(key).ok_or_else(|| Error::MissingKey {
            path: self.key(key),
        })
    }

    fn nested(&self, key: &str) -> Result<Self, Error> {
        match self.get(key)? {
            Variant::Dict(dict) => Ok(Self {
                dict,
                path: self.key(key),
                wide_vectors: self.wide_vectors,
            }),
            _ => Err(wrong_type(self.key(key), "Dictionary")),
        }
    }

    fn array(&self, key: &str) -> Result<&'a [Variant], Error> {
        match self.get(key)? {
            Variant::Array(array) => Ok(&array.items),
            _ => Err(wrong_type(self.key(key), "Array")),
        }
    }

    pub(crate) fn string(&self, key: &str) -> Result<&'a str, Error> {
        match self.get(key)? {
            Variant::String(value) => Ok(value),
            _ => Err(wrong_type(self.key(key), "String")),
        }
    }

    pub(crate) fn integer(&self, key: &str) -> Result<i64, Error> {
        match self.get(key)? {
            Variant::Int(value) => Ok(value.value()),
            _ => Err(wrong_type(self.key(key), "int")),
        }
    }

    fn boolean(&self, key: &str) -> Result<bool, Error> {
        match self.get(key)? {
            Variant::Bool(value) => Ok(*value),
            _ => Err(wrong_type(self.key(key), "bool")),
        }
    }

    pub(crate) fn real(&self, key: &str) -> Result<Real, Error> {
        match self.get(key)? {
            Variant::Float(value) if value.value().is_finite() => Ok(value.value()),
            _ => Err(wrong_type(self.key(key), "finite float")),
        }
    }

    fn vector2(&self, key: &str) -> Result<Vec2, Error> {
        let Variant::Vector2(point) = self.get(key)? else {
            return Err(wrong_type(self.key(key), "Vector2"));
        };
        self.check_width(key, point.width)?;
        let [x, y] = point.values;
        Ok(Vec2::new(x, y))
    }

    pub(crate) fn vector3(&self, key: &str) -> Result<Vec3, Error> {
        let Variant::Vector3(point) = self.get(key)? else {
            return Err(wrong_type(self.key(key), "Vector3"));
        };
        self.check_width(key, point.width)?;
        let [x, y, z] = point.values;
        Ok(Vec3::new(x, y, z))
    }

    fn check_width(&self, key: &str, width: FloatWidth) -> Result<(), Error> {
        if self.wide_vectors && width != FloatWidth::Double {
            return Err(malformed(
                &self.key(key),
                "expected 64-bit vector components",
            ));
        }
        Ok(())
    }

    fn choice<T: Copy>(&self, key: &str, choices: &[T]) -> Result<T, Error> {
        usize::try_from(self.integer(key)?)
            .ok()
            .and_then(|index| choices.get(index).copied())
            .ok_or_else(|| malformed(&self.key(key), "unknown enum value"))
    }
}

fn wrong_type(path: String, expected: &'static str) -> Error {
    Error::WrongType { path, expected }
}

fn malformed(path: &str, message: impl Into<String>) -> Error {
    Error::Malformed {
        path: path.into(),
        message: message.into(),
    }
}

fn vector2(point: Vec2) -> Variant {
    Variant::Vector2(Components {
        values: [point.x, point.y],
        width: FloatWidth::Double,
    })
}
