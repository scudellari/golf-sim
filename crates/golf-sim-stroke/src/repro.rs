//! Exact records of resolved strokes on a course with a constant day.

use golf_sim_base::{
    ClubId, Error, Real, Seed, Vec2, Vec3,
    bundle::BundleEngine,
    corpus::first_difference,
    digest::object,
    trace::Probe,
    variant::{self, Array, Components, Dict, FloatWidth, Variant},
};
use golf_sim_conditions::{grounds::CourseGround, world::World};
use golf_sim_course::course::Course;
use golf_sim_physics::{
    plane::ConstantAir,
    samplers::AirSample,
    values::{Launch, ShotEvent, ShotResult, Touchdown},
};
use golf_sim_swing::execution::Execution;

use crate::{
    address::{Address, Setting},
    delivery::DeliveryInput,
    results::{StrokeOutcome, StrokeResult},
    stroke::{self, StrokeInput},
};

const MAGIC: &[u8; 8] = b"GSSTROK1";
const FORMAT: i64 = 1;

#[derive(Clone, Debug)]
pub struct StrokeSetup {
    pub course_id: String,
    pub course_revision: String,
    pub hole: i64,
    pub tee: Option<String>,
    pub start: Vec3,
    pub cup: Option<Vec3>,
    pub day: ConstantDay,
    pub seeds: StrokeSeeds,
    pub club: ClubId,
    pub address: Address,
    pub aim: Vec3,
}

#[derive(Clone, Copy, Debug)]
pub struct ConstantDay {
    pub clock_s: Real,
    pub air: AirSample,
    pub firmness: Real,
    pub moisture: Real,
    pub green_speed_m: Real,
}

#[derive(Clone, Copy, Debug)]
pub struct StrokeSeeds {
    pub root: Seed,
    pub lie: Seed,
    pub attempt: Seed,
}

impl StrokeSeeds {
    pub fn new(root: Seed) -> Self {
        Self {
            root,
            lie: root.derive("tee"),
            attempt: root.derive("attempt/0"),
        }
    }
}

impl StrokeSetup {
    pub fn world<'a>(&self, course: &'a Course) -> World<'a> {
        let day = self.day;
        let ground = CourseGround::new(course, day.firmness, day.moisture, day.green_speed_m);
        World::on_course(ground, ConstantAir::new(day.air), self.cup, day.clock_s)
    }

    pub fn input(&self, world: &World<'_>, probe: &mut impl Probe) -> StrokeInput {
        let ball = stroke::place(self.start, world, self.seeds.lie, probe);
        let lie = world.address_lie(ball, &stroke::footprint(self.club, &self.address), self.aim);
        StrokeInput::new(
            DeliveryInput {
                club: self.club,
                address: self.address.clone(),
                lie,
                attempt_seed: self.seeds.attempt,
            },
            self.aim,
        )
    }

    fn to_variant(&self) -> Variant {
        object([
            ("course_id", self.course_id.as_str().into()),
            ("course_revision", self.course_revision.as_str().into()),
            ("hole", self.hole.into()),
            ("tee", optional(self.tee.as_deref(), Variant::from)),
            ("start", vec3(self.start)),
            ("cup", optional(self.cup, vec3)),
            ("day", self.day.to_variant()),
            ("seeds", self.seeds.to_variant()),
            ("club", self.club.name().into()),
            ("address", Variant::Dict(self.address.to_dict())),
            ("aim", vec3(self.aim)),
        ])
    }

    fn read(setup: Fields<'_>) -> Result<Self, Error> {
        let address = setup.object("address")?;
        for setting in Setting::ALL {
            address.real(setting.key())?;
        }
        let club = ClubId::from_name(setup.string("club")?)
            .ok_or_else(|| malformed("setup.club", "unknown club"))?;
        let tee = match setup.get("tee")? {
            Variant::Nil => None,
            _ => Some(setup.string("tee")?.into()),
        };
        let aim = setup.point("aim")?;
        if aim == Vec3::ZERO || aim.y != 0.0 {
            return Err(malformed("setup.aim", "expected a horizontal direction"));
        }
        Ok(Self {
            course_id: setup.string("course_id")?.into(),
            course_revision: setup.string("course_revision")?.into(),
            hole: setup.integer("hole")?,
            tee,
            start: setup.point("start")?,
            cup: setup.optional_point("cup")?,
            day: ConstantDay::read(setup.object("day")?)?,
            seeds: StrokeSeeds::read(setup.object("seeds")?)?,
            club,
            address: Address::from_dict(address.dict),
            aim,
        })
    }
}

impl ConstantDay {
    fn to_variant(self) -> Variant {
        let air = self.air;
        object([
            ("clock_s", self.clock_s.into()),
            ("firmness", self.firmness.into()),
            ("moisture", self.moisture.into()),
            ("green_speed_m", self.green_speed_m.into()),
            (
                "air",
                object([
                    ("temperature_c", air.temperature_c.into()),
                    ("pressure_pa", air.pressure_pa.into()),
                    ("relative_humidity", air.relative_humidity.into()),
                    ("mean_wind_mps", vec3(air.mean_wind_mps)),
                    ("gust_mps", vec3(air.gust_mps)),
                    (
                        "wind_reference_height_m",
                        air.wind_reference_height_m.into(),
                    ),
                    ("roughness_length_m", air.roughness_length_m.into()),
                ]),
            ),
        ])
    }

    fn read(day: Fields<'_>) -> Result<Self, Error> {
        let air = day.object("air")?;
        Ok(Self {
            clock_s: day.real("clock_s")?,
            firmness: day.real("firmness")?,
            moisture: day.real("moisture")?,
            green_speed_m: day.real("green_speed_m")?,
            air: AirSample {
                temperature_c: air.real("temperature_c")?,
                pressure_pa: air.real("pressure_pa")?,
                relative_humidity: air.real("relative_humidity")?,
                mean_wind_mps: air.point("mean_wind_mps")?,
                gust_mps: air.point("gust_mps")?,
                wind_reference_height_m: air.real("wind_reference_height_m")?,
                roughness_length_m: air.real("roughness_length_m")?,
            },
        })
    }
}

impl StrokeSeeds {
    fn to_variant(self) -> Variant {
        object([
            ("root", self.root.0.into()),
            ("lie", self.lie.0.into()),
            ("attempt", self.attempt.0.into()),
        ])
    }

    fn read(seeds: Fields<'_>) -> Result<Self, Error> {
        Ok(Self {
            root: Seed(seeds.integer("root")?),
            lie: Seed(seeds.integer("lie")?),
            attempt: Seed(seeds.integer("attempt")?),
        })
    }
}

#[derive(Debug)]
pub struct StrokeRepro {
    pub setup: StrokeSetup,
    pub execution: Execution,
    pub gust_time_s: Real,
    pub flagstick_in: bool,
    versions: Variant,
    expected: Vec<u8>,
}

impl StrokeRepro {
    pub fn capture(
        setup: StrokeSetup,
        input: &StrokeInput,
        execution: Execution,
        result: &StrokeResult,
    ) -> Result<Self, Error> {
        let engine = BundleEngine::current();
        Ok(Self {
            setup,
            execution,
            gust_time_s: input.gust_time_s,
            flagstick_in: input.flagstick_in,
            versions: object([
                ("release", engine.release.as_str().into()),
                ("commit", engine.commit.as_str().into()),
                ("physics", golf_sim_physics::tunables::VERSION.into()),
                ("stroke", crate::tuning::VERSION.into()),
            ]),
            expected: variant::encode(&result_variant(result))?,
        })
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let body = variant::encode(&object([
            ("format", FORMAT.into()),
            ("versions", self.versions.clone()),
            ("setup", self.setup.to_variant()),
            (
                "execution",
                object([
                    ("power", self.execution.power.into()),
                    ("timing_fault", self.execution.timing_fault.into()),
                    ("timed_out", Variant::Bool(self.execution.timed_out)),
                    ("strike_m", optional(self.execution.strike_m, vec2)),
                ]),
            ),
            ("gust_time_s", self.gust_time_s.into()),
            ("flagstick_in", Variant::Bool(self.flagstick_in)),
            ("result", Variant::PackedByteArray(self.expected.clone())),
        ]))?;
        let size =
            u32::try_from(body.len()).map_err(|_| malformed("record.bin", "record too large"))?;
        let mut bytes = Vec::from(*MAGIC);
        bytes.extend(size.to_le_bytes());
        bytes.extend(body);
        Ok(bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let (size, body) = bytes
            .strip_prefix(MAGIC)
            .and_then(|bytes| bytes.split_first_chunk::<4>())
            .ok_or_else(|| malformed("record.bin", "missing GSSTROK1 header"))?;
        if body.len() != u32::from_le_bytes(*size) as usize {
            return Err(malformed("record.bin", "length does not match header"));
        }
        let value = variant::decode(body)?;
        let record = Fields::new(&value, "")?;
        let format = record.integer("format")?;
        if format != FORMAT {
            return Err(Error::UnknownVersion {
                what: "stroke repro",
                version: format,
            });
        }
        let versions = record.object("versions")?;
        for name in ["release", "commit", "physics", "stroke"] {
            versions.string(name)?;
        }
        let execution = record.object("execution")?;
        let strike_m = match execution.get("strike_m")? {
            Variant::Nil => None,
            Variant::Vector2(point) if point.values.iter().all(|value| value.is_finite()) => {
                let [x, y] = point.values;
                Some(Vec2::new(x, y))
            }
            _ => return Err(execution.wrong_type("strike_m", "finite Vector2 or nil")),
        };
        let Variant::PackedByteArray(expected) = record.get("result")? else {
            return Err(record.wrong_type("result", "PackedByteArray"));
        };
        Ok(Self {
            setup: StrokeSetup::read(record.object("setup")?)?,
            execution: Execution {
                power: execution.real("power")?,
                timing_fault: execution.real("timing_fault")?,
                timed_out: execution.boolean("timed_out")?,
                strike_m,
                // Format 1 stores only the fields delivery uses.
                timing_error_s: 0.0,
                impact_usec: 0,
                backswing_s: 0.0,
                ring_s: 0.0,
            },
            gust_time_s: record.real("gust_time_s")?,
            flagstick_in: record.boolean("flagstick_in")?,
            versions: record.get("versions")?.clone(),
            expected: expected.clone(),
        })
    }

    pub fn replay(&self, course: &Course, probe: &mut impl Probe) -> Result<StrokeResult, Error> {
        if course.revision() != self.setup.course_revision {
            return Err(Error::RevisionMismatch {
                expected: self.setup.course_revision.clone(),
                actual: course.revision().into(),
            });
        }
        if course.data().id != self.setup.course_id {
            return Err(malformed(
                "setup.course_id",
                "record names a different course",
            ));
        }
        let world = self.setup.world(course);
        let mut input = self.setup.input(&world, probe);
        input.gust_time_s = self.gust_time_s;
        input.flagstick_in = self.flagstick_in;
        let result = stroke::play(&input, self.execution, &world, probe);
        let actual = result_variant(&result);
        if self.expected != variant::encode(&actual)? {
            let expected = variant::decode(&self.expected)
                .map_err(|error| malformed("result", error.to_string()))?;
            let path = first_difference(&expected, &actual)
                .filter(|path| path != "$")
                .map_or_else(|| "result".into(), |path| format!("result.{path}"));
            return Err(Error::Diverged { entry: 1, path });
        }
        Ok(result)
    }
}

struct Fields<'a> {
    dict: &'a Dict,
    path: String,
}

impl<'a> Fields<'a> {
    fn new(value: &'a Variant, path: impl Into<String>) -> Result<Self, Error> {
        let path = path.into();
        let dict = value.as_dict().ok_or_else(|| Error::WrongType {
            path: if path.is_empty() {
                "$".into()
            } else {
                path.clone()
            },
            expected: "Dictionary",
        })?;
        Ok(Self { dict, path })
    }

    fn key(&self, name: &str) -> String {
        if self.path.is_empty() {
            name.into()
        } else {
            format!("{}.{name}", self.path)
        }
    }

    fn wrong_type(&self, name: &str, expected: &'static str) -> Error {
        Error::WrongType {
            path: self.key(name),
            expected,
        }
    }

    fn get(&self, name: &str) -> Result<&'a Variant, Error> {
        self.dict.get(name).ok_or_else(|| Error::MissingKey {
            path: self.key(name),
        })
    }

    fn object(&self, name: &str) -> Result<Self, Error> {
        Self::new(self.get(name)?, self.key(name))
    }

    fn string(&self, name: &str) -> Result<&'a str, Error> {
        self.get(name)?
            .as_str()
            .ok_or_else(|| self.wrong_type(name, "String"))
    }

    fn integer(&self, name: &str) -> Result<i64, Error> {
        match self.get(name)? {
            Variant::Int(value) => Ok(value.value()),
            _ => Err(self.wrong_type(name, "int")),
        }
    }

    fn boolean(&self, name: &str) -> Result<bool, Error> {
        match self.get(name)? {
            Variant::Bool(value) => Ok(*value),
            _ => Err(self.wrong_type(name, "bool")),
        }
    }

    fn real(&self, name: &str) -> Result<Real, Error> {
        match self.get(name)? {
            Variant::Float(value) if value.value().is_finite() => Ok(value.value()),
            _ => Err(self.wrong_type(name, "finite float")),
        }
    }

    fn point(&self, name: &str) -> Result<Vec3, Error> {
        match self.get(name)? {
            Variant::Vector3(point) if point.values.iter().all(|value| value.is_finite()) => {
                let [x, y, z] = point.values;
                Ok(Vec3::new(x, y, z))
            }
            _ => Err(self.wrong_type(name, "finite Vector3")),
        }
    }

    fn optional_point(&self, name: &str) -> Result<Option<Vec3>, Error> {
        match self.get(name)? {
            Variant::Nil => Ok(None),
            _ => self.point(name).map(Some),
        }
    }
}

fn malformed(path: &str, message: impl Into<String>) -> Error {
    Error::Malformed {
        path: path.into(),
        message: message.into(),
    }
}

fn vec2(point: Vec2) -> Variant {
    Variant::Vector2(Components {
        values: [point.x, point.y],
        width: FloatWidth::Double,
    })
}

fn xyz(point: Vec3) -> [Real; 3] {
    [point.x, point.y, point.z]
}

fn vec3(point: Vec3) -> Variant {
    Variant::Vector3(Components {
        values: xyz(point),
        width: FloatWidth::Double,
    })
}

fn optional<T>(value: Option<T>, encode: impl FnOnce(T) -> Variant) -> Variant {
    value.map_or(Variant::Nil, encode)
}

fn array(values: impl Iterator<Item = Variant>) -> Variant {
    Variant::Array(Array::new(values.collect()))
}

fn result_variant(result: &StrokeResult) -> Variant {
    object([
        ("playback", playback_variant(&result.playback)),
        ("outcome", outcome_variant(&result.outcome)),
        ("fault", result.fault.into()),
        ("quality", (result.quality as i64).into()),
        ("fat", Variant::Bool(result.fat)),
    ])
}

fn playback_variant(shot: &ShotResult) -> Variant {
    object([
        ("launch", optional(shot.launch, launch_variant)),
        ("times", Variant::PackedFloat64Array(shot.times.clone())),
        (
            "positions",
            Variant::PackedVector3Array(Components {
                values: shot.positions.iter().copied().map(xyz).collect(),
                width: FloatWidth::Double,
            }),
        ),
        ("events", array(shot.events.iter().map(event_variant))),
        (
            "touchdowns",
            array(shot.touchdowns.iter().map(touchdown_variant)),
        ),
        ("end_reason", (shot.end_reason as i64).into()),
        ("rest_position", vec3(shot.rest_position)),
        ("rest_surface", shot.rest_surface.name().into()),
        ("embedded", Variant::Bool(shot.embedded)),
    ])
}

fn launch_variant(launch: Launch) -> Variant {
    let readout = launch.readout;
    object([
        ("position", vec3(launch.position)),
        ("velocity", vec3(launch.velocity)),
        ("spin", vec3(launch.spin)),
        ("aim", vec3(launch.aim)),
        (
            "readout",
            object([
                ("ball_speed_mps", readout.ball_speed_mps.into()),
                ("launch_angle_rad", readout.launch_angle_rad.into()),
                ("launch_direction_rad", readout.launch_direction_rad.into()),
                ("spin_rate_rad_s", readout.spin_rate_rad_s.into()),
                ("spin_axis_rad", readout.spin_axis_rad.into()),
            ]),
        ),
        ("club_speed_mps", launch.club_speed_mps.into()),
        (
            "club_speed_at_ball_mps",
            launch.club_speed_at_ball_mps.into(),
        ),
        ("smash_factor", launch.smash_factor.into()),
        ("club_path_rad", launch.club_path_rad.into()),
        ("attack_angle_rad", launch.attack_angle_rad.into()),
        ("face_angle_rad", launch.face_angle_rad.into()),
        ("dynamic_loft_rad", launch.dynamic_loft_rad.into()),
        ("face_to_path_rad", launch.face_to_path_rad.into()),
        ("spin_loft_rad", launch.spin_loft_rad.into()),
        ("valid", Variant::Bool(launch.valid)),
        ("contact", (launch.contact as i64).into()),
        ("strike_toe_m", launch.strike_toe_m.into()),
        ("strike_up_m", launch.strike_up_m.into()),
        ("low_point_ahead_m", launch.low_point_ahead_m.into()),
        ("low_point_height_m", launch.low_point_height_m.into()),
        ("turf_entry_behind_m", launch.turf_entry_behind_m.into()),
        (
            "turf_contact_start",
            optional(launch.turf_contact_start, vec3),
        ),
        ("turf_contact_end", optional(launch.turf_contact_end, vec3)),
        ("turf_contact_depth_m", launch.turf_contact_depth_m.into()),
        ("interference", launch.interference.into()),
        ("ball_temperature_c", launch.ball_temperature_c.into()),
    ])
}

fn event_variant(event: &ShotEvent) -> Variant {
    object([
        ("kind", (event.kind as i64).into()),
        ("time_s", event.time_s.into()),
        ("position", vec3(event.position)),
        ("velocity", vec3(event.velocity)),
        ("spin", vec3(event.spin)),
        ("surface", event.surface.name().into()),
        ("zone", (event.zone as i64).into()),
    ])
}

fn touchdown_variant(touch: &Touchdown) -> Variant {
    object([
        ("time_s", touch.time_s.into()),
        ("position", vec3(touch.position)),
        ("contact_point", vec3(touch.contact_point)),
        ("velocity", vec3(touch.velocity)),
        ("depth_m", touch.depth_m.into()),
        ("surface", touch.surface.name().into()),
    ])
}

fn outcome_variant(outcome: &StrokeOutcome) -> Variant {
    object([
        ("end", (outcome.end as i64).into()),
        ("start", vec3(outcome.start)),
        ("rest", vec3(outcome.rest)),
        ("rest_surface", outcome.rest_surface.name().into()),
        ("ball_radius_m", outcome.ball_radius_m.into()),
        ("carry_m", outcome.carry_m.into()),
        ("total_m", outcome.total_m.into()),
        ("embedded", Variant::Bool(outcome.embedded)),
        ("penalty_entry", optional(outcome.penalty_entry, vec3)),
        (
            "marks",
            array(outcome.marks.iter().map(|mark| {
                object([
                    ("kind", (mark.kind as i64).into()),
                    ("start", vec3(mark.start)),
                    ("end", vec3(mark.end)),
                    ("depth_m", mark.depth_m.into()),
                    ("surface", mark.surface.name().into()),
                ])
            })),
        ),
    ])
}
