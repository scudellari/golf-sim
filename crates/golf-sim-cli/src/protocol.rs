//! Wire types for the JSON-lines hole session.

use std::{
    fs::{self, File},
    io::{self, Write},
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};

use clap::ValueEnum;
use golf_sim::{
    action::{Action, ReliefOption, StrokeChoice, Swing},
    base::{ClubId, PromptKind, Real, Refusal, Surface, Vec2, Vec3},
    course::course::Course,
    hole::{Hole, prompt::ActionRefusal},
    observe::{
        AddressReading, Distance, GreenReading, Observation, ReliefOffer, Score, ShotReport, Wind,
    },
    setup::Mode,
    stroke::results::{Aloft, End, Preview, StrokeOutcome},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const VERSION: u32 = 1;

type Point = [Real; 3];

macro_rules! wire_enum {
    ($name:ident, $core:literal, $($variant:ident $(as $rename:literal)?),+ $(,)?) => {
        #[derive(Serialize, Deserialize, JsonSchema)]
        #[serde(remote = $core, rename_all = "snake_case")]
        #[schemars(rename = $core)]
        enum $name {
            $($(#[serde(rename = $rename)])? $variant),+
        }
    };
}

wire_enum!(WireClub, "ClubId", Driver, Wood3 as "3wood", Hybrid, Iron4 as "4iron",
    Iron5 as "5iron", Iron6 as "6iron", Iron7 as "7iron", Iron8 as "8iron", Iron9 as "9iron",
    PitchingWedge as "pw", GapWedge as "gw", SandWedge as "sw", LobWedge as "lw", Putter);
wire_enum!(
    WireRelief,
    "ReliefOption",
    Cancel,
    StrokeAndDistance,
    BackOnLine,
    Lateral,
    OutsideBunker,
    TemporaryWater,
    GroundUnderRepair,
    ImmovableObstruction,
    Embedded,
    BunkerAbnormalOutside,
    Provisional
);
wire_enum!(
    WireSurface,
    "Surface",
    Tee,
    Fairway,
    FirstCut,
    Rough,
    DeepRough,
    Fringe,
    Green,
    Bunker,
    Hardpan,
    CartPath,
    Water
);
wire_enum!(
    WirePrompt,
    "PromptKind",
    Play,
    MaybeLost,
    Choose,
    Drop,
    Place,
    Done
);
wire_enum!(WireMode, "Mode", Caddie, Tour);
wire_enum!(WireEnd, "End", AtRest, Holed, InWater, TimedOut);
wire_enum!(WireAloft, "Aloft", Same, More, Less);

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum ProfileName {
    Scratch,
    Hcp15,
}

impl ProfileName {
    pub fn swing(self) -> Swing {
        let Swing::Skill(mut profile) = StrokeChoice::stock(ClubId::Driver, Vec3::ZERO).swing
        else {
            unreachable!("stock choice uses a skill profile");
        };
        if matches!(self, Self::Hcp15) {
            profile.name = "hcp15";
            profile.sigma_p_ms = 30.0;
            profile.bias_t_ms = 0.0;
            profile.sigma_t_ms = 15.0;
            profile.sigma_s_mm = 5.0;
            profile.tau_ms = 80.0;
        }
        Swing::Skill(profile)
    }
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct Request {
    pub id: Value,
    #[serde(flatten)]
    pub operation: Operation,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Observe,
    Legal,
    Preview { stroke: AimChoice },
    Act { action: Decision },
    Record,
    Save { path: String },
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Decision {
    Stroke(Stroke),
    GoForward {},
    Relief {
        #[serde(with = "WireRelief")]
        option: ReliefOption,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        footprint: Vec<[Real; 2]>,
    },
    Drop {
        at: Point,
    },
    Place {
        at: Point,
    },
}

impl Decision {
    pub fn action(self, course: &Course, profile: Option<ProfileName>) -> Result<Action, Refused> {
        Ok(match self {
            Self::Stroke(stroke) => {
                let swing = stroke.swing.or_else(|| profile.map(SwingChoice::Profile));
                let swing = swing.ok_or(Refused::MissingSwing {})?;
                Action::Stroke(stroke.aim.choice(course).with_swing(swing.into()))
            }
            Self::GoForward {} => Action::GoForward,
            Self::Relief { option, footprint } => Action::Relief {
                option,
                footprint: footprint.into_iter().map(vec2).collect(),
            },
            Self::Drop { at } => Action::Drop(vec3(at)),
            Self::Place { at } => Action::Place(vec3(at)),
        })
    }

    pub fn offered(action: Action, line: Option<(Vec3, Vec3)>) -> Self {
        match action {
            Action::Stroke(choice) => Self::Stroke(Stroke {
                aim: AimChoice {
                    club: choice.club,
                    // Hole::legal supplies a suggested stroke in its first aim direction.
                    aim_at: AimPoint::Space({
                        let (ball, direction) = line.expect("a stroke has an aim");
                        point(ball + direction)
                    }),
                },
                swing: None,
            }),
            Action::GoForward => Self::GoForward {},
            Action::Relief { option, footprint } => Self::Relief {
                option,
                footprint: footprint.into_iter().map(|p| [p.x, p.y]).collect(),
            },
            Action::Drop(at) => Self::Drop { at: point(at) },
            Action::Place(at) => Self::Place { at: point(at) },
        }
    }
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AimChoice {
    #[serde(with = "WireClub")]
    pub club: ClubId,
    pub aim_at: AimPoint,
}

impl AimChoice {
    pub fn choice(self, course: &Course) -> StrokeChoice {
        let target = match self.aim_at {
            AimPoint::Ground([x, z]) => Vec3::new(x, course.height_at(Vec3::new(x, 0.0, z)), z),
            AimPoint::Space(at) => vec3(at),
        };
        StrokeChoice::stock(self.club, target)
    }
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum AimPoint {
    Ground([Real; 2]),
    Space(Point),
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct Stroke {
    #[serde(flatten)]
    aim: AimChoice,
    #[serde(skip_serializing_if = "Option::is_none")]
    swing: Option<SwingChoice>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum SwingChoice {
    Exact(ExactSwing),
    Profile(ProfileName),
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExactSwing {
    power: Real,
    timing_fault: Real,
    strike_m: Option<[Real; 2]>,
}

impl From<SwingChoice> for Swing {
    fn from(choice: SwingChoice) -> Self {
        match choice {
            SwingChoice::Exact(exact) => Self::Exact {
                power: exact.power,
                timing_fault: exact.timing_fault,
                strike_m: exact.strike_m.map(vec2),
            },
            SwingChoice::Profile(profile) => profile.swing(),
        }
    }
}

#[derive(JsonSchema, Serialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Reply {
    Ok { id: Value, ok: Success },
    Refused { id: Value, refused: Refused },
}

#[derive(JsonSchema, Serialize)]
#[serde(untagged)]
pub enum Success {
    Observe(Box<Observed>),
    Legal { actions: Vec<Decision> },
    Preview(Previewed),
    Act(Acted),
    Record { bytes: Vec<u8> },
    Save { path: String },
}

#[derive(JsonSchema, Serialize)]
pub struct Observed {
    protocol: u32,
    prompt: Prompt,
    hole: HoleInfo,
    ball: Option<Ball>,
    to_cup_m: Option<Real>,
    strokes: usize,
    wind: Option<Wind>,
    advice: Option<Advice>,
    #[serde(with = "WireMode")]
    mode: Mode,
    score: Score,
    cup: Point,
    to_flag: Option<Distance>,
    flag_rise_m: Option<Real>,
    address: Option<AddressReading>,
    green: Option<GreenReading>,
    relief_available: bool,
    relief_offers: Vec<ReliefOffer>,
    last_stroke: Option<ShotReport>,
}

#[derive(JsonSchema, Serialize)]
struct HoleInfo {
    number: i64,
    par: i64,
}

#[derive(JsonSchema, Serialize)]
struct Ball {
    spot: Point,
    #[serde(with = "WireSurface")]
    lie: Surface,
}

#[derive(JsonSchema, Serialize)]
struct Advice {
    #[serde(with = "WireClub")]
    club: ClubId,
    aim_point: Point,
}

impl From<Observation> for Observed {
    fn from(view: Observation) -> Self {
        let advice = view.address.as_ref().map(|address| Advice {
            club: address.club,
            aim_point: address.aim_point,
        });
        Self {
            protocol: VERSION,
            prompt: view.prompt.into(),
            hole: HoleInfo {
                number: view.score.number,
                par: view.score.par,
            },
            ball: view.ball.map(|ball| Ball {
                spot: ball.spot,
                lie: ball.surface,
            }),
            to_cup_m: view.to_cup.map(|distance| distance.exact_m),
            strokes: view.score.strokes,
            wind: view.wind,
            advice,
            mode: view.mode,
            score: view.score,
            cup: view.cup,
            to_flag: view.to_flag,
            flag_rise_m: view.flag_rise_m,
            address: view.address,
            green: view.green,
            relief_available: view.relief_available,
            relief_offers: view.relief_offers,
            last_stroke: view.last_stroke,
        }
    }
}

#[derive(JsonSchema, Serialize)]
pub struct Previewed {
    carry_m: Option<Real>,
    plays_like_m: Option<Real>,
    #[serde(with = "WireAloft")]
    wind_aloft: Aloft,
    roll_m: Option<Real>,
    target_distance_m: Option<Real>,
    carry_to_target_m: Option<Real>,
    wind_aloft_mps: Point,
    wind_here_mps: Point,
    putt_line: Vec<Point>,
}

impl From<Preview> for Previewed {
    fn from(preview: Preview) -> Self {
        let finite = |value: Real| value.is_finite().then_some(value);
        Self {
            carry_m: finite(preview.carry_m),
            plays_like_m: finite(preview.plays_like_m),
            wind_aloft: preview.aloft,
            roll_m: finite(preview.roll_m),
            target_distance_m: finite(preview.target_distance_m),
            carry_to_target_m: finite(preview.carry_to_target_m),
            wind_aloft_mps: point(preview.wind_aloft_mps),
            wind_here_mps: point(preview.wind_here_mps),
            putt_line: preview.putt_line.into_iter().map(point).collect(),
        }
    }
}

#[derive(JsonSchema, Serialize)]
pub struct Acted {
    stroke: Option<PlayedStroke>,
    ruling: Option<Ruling>,
    prompt: Prompt,
}

impl Acted {
    pub fn new(hole: &Hole, stroke: Option<StrokeOutcome>) -> Self {
        let prompt = hole.prompt();
        let ruling = prompt
            .ruling
            .filter(|ruling| {
                prompt.kind != PromptKind::Play || ruling.penalty_strokes != 0 || stroke.is_none()
            })
            .map(|ruling| Ruling {
                penalty_strokes: ruling.penalty_strokes,
                spot: ruling.spot.map(point),
                place_at: ruling.place_at.map(point),
            });
        Self {
            stroke: stroke.map(|stroke| PlayedStroke {
                end: stroke.end,
                carry_m: stroke.carry_m,
                total_m: stroke.total_m,
                lie: stroke.rest_surface,
                rest: point(stroke.rest),
            }),
            ruling,
            prompt: prompt.kind.into(),
        }
    }
}

#[derive(JsonSchema, Serialize)]
struct PlayedStroke {
    #[serde(with = "WireEnd")]
    end: End,
    carry_m: Real,
    total_m: Real,
    #[serde(with = "WireSurface")]
    lie: Surface,
    rest: Point,
}

#[derive(JsonSchema, Serialize)]
struct Ruling {
    penalty_strokes: usize,
    spot: Option<Point>,
    place_at: Option<Point>,
}

#[derive(JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
enum Prompt {
    Play {},
    MaybeLost {},
    Choose {},
    Drop {},
    Place {},
    Done {},
}

impl From<PromptKind> for Prompt {
    fn from(kind: PromptKind) -> Self {
        match kind {
            PromptKind::Play => Self::Play {},
            PromptKind::MaybeLost => Self::MaybeLost {},
            PromptKind::Choose => Self::Choose {},
            PromptKind::Drop => Self::Drop {},
            PromptKind::Place => Self::Place {},
            PromptKind::Done => Self::Done {},
        }
    }
}

#[derive(JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Refused {
    Malformed {
        message: String,
    },
    UnknownOp {
        op: String,
    },
    NotNow {
        #[serde(with = "WirePrompt")]
        prompt: PromptKind,
    },
    MissingSwing {},
    PreviewUnavailable {},
    Invalid {
        #[serde(with = "WirePrompt")]
        prompt: PromptKind,
        message: String,
    },
    BadPath {
        path: String,
        message: String,
    },
    Record {
        message: String,
    },
}

impl Refused {
    pub fn action(error: ActionRefusal, prompt: PromptKind) -> Self {
        match error {
            ActionRefusal::Rules(Refusal::NotNow { prompt }) => Self::NotNow { prompt },
            ActionRefusal::PreviewUnavailable => Self::PreviewUnavailable {},
            _ => Self::Invalid {
                prompt,
                message: error.to_string(),
            },
        }
    }
}

pub fn read_request(line: &[u8]) -> Result<Request, (Value, Refused)> {
    serde_json::from_slice(line).map_err(|error| {
        let envelope = serde_json::from_slice::<Value>(line).unwrap_or(Value::Null);
        let id = envelope.get("id").cloned().unwrap_or(Value::Null);
        let refused = match envelope.get("op").and_then(Value::as_str) {
            Some(op)
                if !matches!(
                    op,
                    "observe" | "legal" | "preview" | "act" | "record" | "save"
                ) =>
            {
                Refused::UnknownOp { op: op.into() }
            }
            _ => Refused::Malformed {
                message: error.to_string(),
            },
        };
        (id, refused)
    })
}

pub fn write_line(out: &mut impl Write, line: &impl Serialize) -> std::io::Result<()> {
    serde_json::to_writer(&mut *out, line)?;
    writeln!(out)
}

pub fn record(hole: &Hole) -> Result<Vec<u8>, Refused> {
    hole.record().to_bytes().map_err(|error| Refused::Record {
        message: error.to_string(),
    })
}

pub fn save(hole: &Hole, path: String) -> Result<Success, Refused> {
    write_record(Path::new(&path), &record(hole)?).map_err(|error| Refused::BadPath {
        path: path.clone(),
        message: error.to_string(),
    })?;
    Ok(Success::Save { path })
}

fn write_record(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static SERIAL: AtomicUsize = AtomicUsize::new(0);
    if path.file_name().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a file path",
        ));
    }
    let staged = path.with_file_name(format!(
        ".golf-play-{}-{}.tmp",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = File::create_new(&staged)?;
    let written = file.write_all(bytes);
    drop(file);
    let result = written.and_then(|()| fs::rename(&staged, path));
    if result.is_err() {
        let _ = fs::remove_file(staged);
    }
    result
}

fn point(vec: Vec3) -> Point {
    [vec.x, vec.y, vec.z]
}

fn vec3([x, y, z]: Point) -> Vec3 {
    Vec3::new(x, y, z)
}

fn vec2([x, y]: [Real; 2]) -> Vec2 {
    Vec2::new(x, y)
}

#[cfg(test)]
mod tests {
    use golf_sim::base::{Seed, seed::SeedStream};

    use super::*;

    #[test]
    fn a_million_doubles_round_trip_as_exact_swings() {
        let mut random = SeedStream::new(Seed(7));
        let mut finite_double = || loop {
            let value = Real::from_bits(random.next_int() as u64);
            if value.is_finite() {
                break value;
            }
        };
        let edges = [
            [0.0, -0.0, Real::MAX, Real::MIN],
            [
                Real::from_bits(1),
                Real::from_bits((1 << 63) | 1),
                Real::from_bits(Real::MIN_POSITIVE.to_bits() - 1),
                Real::MIN_POSITIVE,
            ],
        ];
        let mut line = Vec::new();
        for values in edges.into_iter().chain(
            std::iter::repeat_with(|| std::array::from_fn(|_| finite_double())).take(250_000),
        ) {
            let [power, timing_fault, x, y] = values;
            let request = Request {
                id: Value::Null,
                operation: Operation::Act {
                    action: Decision::Stroke(Stroke {
                        aim: AimChoice {
                            club: ClubId::Driver,
                            aim_at: AimPoint::Ground([x, y]),
                        },
                        swing: Some(SwingChoice::Exact(ExactSwing {
                            power,
                            timing_fault,
                            strike_m: Some([x, y]),
                        })),
                    }),
                },
            };
            line.clear();
            write_line(&mut line, &request).unwrap();
            let Ok(Request {
                operation:
                    Operation::Act {
                        action:
                            Decision::Stroke(Stroke {
                                aim:
                                    AimChoice {
                                        aim_at: AimPoint::Ground(aim),
                                        ..
                                    },
                                swing:
                                    Some(SwingChoice::Exact(ExactSwing {
                                        power,
                                        timing_fault,
                                        strike_m: Some([x, y]),
                                    })),
                            }),
                    },
                ..
            }) = read_request(&line)
            else {
                panic!(
                    "request did not round-trip: {}",
                    String::from_utf8_lossy(&line)
                );
            };
            assert_eq!(
                [power, timing_fault, x, y].map(Real::to_bits),
                values.map(Real::to_bits)
            );
            assert_eq!(aim.map(Real::to_bits), [x, y].map(Real::to_bits));
        }
        println!("1,000,008 exact-swing doubles preserved every bit");
    }
}
