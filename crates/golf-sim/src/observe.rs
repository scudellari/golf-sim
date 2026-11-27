//! The numeric readings at a hole's current prompt.

use golf_sim_base::{ClubId, PromptKind, Real, Seed, Surface, Vec2, Vec3, fm, geometry};
use golf_sim_conditions::{
    lies::{AddressLie, BallLie, DivotLie, Sit},
    world::World,
};
use golf_sim_physics::{
    facade::standard_club,
    samplers::GroundSampler,
    values::{
        Contact,
        units::{FOOT, MPH, YARD},
    },
};
use golf_sim_rules::rulings::{ReliefOption, Status};
use golf_sim_stroke::{
    address::Address,
    bag::{self, Group},
    delivery::DeliveryInput,
    results::{Aloft, End, Preview, StrikeQuality, StrokeResult},
    stroke::{self, StrokeInput},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    action::Action,
    hole::Hole,
    records::hole::{Entry, StrokeFacts},
    setup::Mode,
};

const PLAYS_LIKE_STEP_M: Real = 5.0 * YARD;
const PUTTING_DISTANCE_M: Real = 12.0;
const LANDING_SHORT_OF_FLAG_M: Real = 25.0;
const WIND_HEIGHTS_M: [Real; 6] = [0.01, 0.03, 0.06, 0.15, 0.4, 1.0];

type Point = [Real; 3];

// Keep the core enums in the Rust API and define their wire names at its boundary.
macro_rules! wire_enum {
    ($name:ident, $remote:literal, $($variant:ident $(as $rename:literal)?),+ $(,)?) => {
        #[derive(Serialize, Deserialize, JsonSchema)]
        #[serde(remote = $remote, rename_all = "snake_case")]
        #[schemars(rename = $remote)]
        enum $name {
            $($(#[serde(rename = $rename)])? $variant),+
        }
    };
}

wire_enum!(WireMode, "Mode", Caddie, Tour);
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
wire_enum!(WireClub, "ClubId", Driver, Wood3 as "3wood", Hybrid, Iron4 as "4iron",
    Iron5 as "5iron", Iron6 as "6iron", Iron7 as "7iron", Iron8 as "8iron", Iron9 as "9iron",
    PitchingWedge as "pw", GapWedge as "gw", SandWedge as "sw", LobWedge as "lw", Putter);
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
wire_enum!(WireSit, "Sit", Up, Normal, Down);
wire_enum!(WireDivot, "DivotLie", None, Fresh, Old, SandFilled);
wire_enum!(WireAloft, "Aloft", Same, More, Less);
wire_enum!(WireEnd, "End", AtRest, Holed, InWater, TimedOut);
wire_enum!(WireQuality, "StrikeQuality", Pure, Solid, Mishit);
wire_enum!(
    WireContact,
    "Contact",
    Face,
    LeadingEdge,
    TopEdge,
    Sand,
    SideEdge,
    Miss
);
wire_enum!(
    WireStatus,
    "Status",
    Holed,
    OutOfBounds,
    Lost,
    InWater,
    InPlay,
    PenaltyArea,
    Doubtful
);
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Observation {
    #[serde(with = "WireMode")]
    pub mode: Mode,
    #[serde(with = "WirePrompt")]
    pub prompt: PromptKind,
    pub score: Score,
    pub cup: Point,
    pub ball: Option<LieReading>,
    pub to_flag: Option<Distance>,
    pub to_cup: Option<Distance>,
    /// The rangefinder aimed at the flag. Only Caddie mode displays the rise.
    pub flag_rise_m: Option<Real>,
    pub wind: Option<Wind>,
    pub address: Option<AddressReading>,
    pub green: Option<GreenReading>,
    pub relief_available: bool,
    pub relief_offers: Vec<ReliefOffer>,
    pub last_stroke: Option<ShotReport>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Score {
    pub number: i64,
    pub par: i64,
    pub length_yd: i64,
    pub strokes: usize,
    pub next_stroke: Option<usize>,
    pub penalties: usize,
    pub to_par: i64,
    pub provisional: bool,
    pub card: Vec<CardStroke>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CardStroke {
    pub number: usize,
    #[serde(with = "WireClub")]
    pub club: ClubId,
    pub from: Point,
    pub rest: Point,
    pub distance: Distance,
    #[serde(with = "WireSurface")]
    pub rest_surface: Surface,
    #[serde(with = "WireStatus")]
    pub status: Status,
    pub penalties: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Distance {
    pub exact_m: Real,
    pub display: DistanceDisplay,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DistanceDisplay {
    Yards(i64),
    TenthsOfFeet(i64),
}

impl Distance {
    fn new(exact_m: Real, feet: bool) -> Self {
        let display = if feet {
            DistanceDisplay::TenthsOfFeet((exact_m / FOOT * 10.0).round() as i64)
        } else {
            DistanceDisplay::Yards((exact_m / YARD).round() as i64)
        };
        Self { exact_m, display }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LieReading {
    pub spot: Point,
    #[serde(with = "WireSurface")]
    pub surface: Surface,
    #[serde(with = "WireSit")]
    pub sit: Sit,
    #[serde(with = "WireDivot")]
    pub divot: DivotLie,
    pub ball_height_m: Real,
    pub grass_height_m: Real,
    pub grass_density: Real,
    pub sand_firmness: Real,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Wind {
    pub felt_vector_mps: Point,
    pub felt_mps: Real,
    pub felt_mph: i64,
    pub from_deg: Option<Real>,
    pub along_aim: Option<AlongWind>,
    pub across_aim: Option<CrossWind>,
    pub flag_mps: Point,
    pub under_trees: bool,
    pub ground_mps: Point,
    /// Wind at 0.01, 0.03, 0.06, 0.15, 0.4 and 1 metre above the ground.
    pub at_heights_mps: Vec<Point>,
    pub temperature_c: Option<Real>,
    pub displayed_temperature_c: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AlongWind {
    Into,
    Helping,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CrossWind {
    FromLeft,
    FromRight,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AddressReading {
    #[serde(with = "WireClub")]
    pub club: ClubId,
    pub club_length_m: Real,
    pub club_lie_rad: Real,
    pub face_half_width_m: Real,
    pub face_height_m: Real,
    pub face_is_wood: bool,
    pub putting: bool,
    pub aim: Point,
    pub aim_point: Point,
    pub flagstick_in: bool,
    pub stock_carry: Option<Distance>,
    pub landing: Option<Point>,
    pub stance: Stance,
    pub preview: Option<PreviewReading>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Stance {
    pub slope_along_rad: Real,
    pub slope_across_rad: Real,
    pub ball_slope_along_rad: Real,
    /// Positive is uphill along the line or ball above feet across it.
    pub along_deg: i64,
    pub across_deg: i64,
    #[serde(with = "WireSurface")]
    pub lead_foot_surface: Surface,
    #[serde(with = "WireSurface")]
    pub trail_foot_surface: Surface,
    #[serde(with = "WireSurface")]
    pub club_entry_surface: Surface,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PreviewReading {
    pub plays_like_m: Option<Real>,
    /// The Caddie's nearest-five-yard read when the adjustment is noticeable.
    pub displayed_plays_like_yd: Option<i64>,
    pub carry: Option<Distance>,
    pub carry_to_target_m: Option<Real>,
    pub nominal_carry: Option<Distance>,
    pub nominal_roll: Option<Distance>,
    pub putt_line: Vec<Point>,
    #[serde(with = "WireAloft")]
    pub aloft: Aloft,
    pub wind_aloft_mps: Point,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GreenReading {
    pub fall: Point,
    pub overlay: Option<GreenOverlay>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GreenOverlay {
    pub speed_m: Real,
    pub samples: Vec<GreenSample>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GreenSample {
    pub point: Point,
    pub fall: Point,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReliefOffer {
    #[serde(with = "WireRelief")]
    pub option: ReliefOption,
    pub penalty_strokes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShotCause {
    None,
    Timing,
    Strike,
    Power,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ShotReport {
    #[serde(with = "WireClub")]
    pub club: ClubId,
    #[serde(with = "WireEnd")]
    pub end: End,
    #[serde(with = "WireQuality")]
    pub quality: StrikeQuality,
    #[serde(with = "WireContact")]
    pub contact: Contact,
    pub cause: ShotCause,
    pub carry: Distance,
    pub total: Distance,
    pub offline: Distance,
    pub total_offline: Distance,
    pub proximity: Distance,
    pub apex_m: Real,
    pub hang_time_s: Real,
    pub land_angle_rad: Real,
    pub ball_speed_mps: Real,
    pub launch_angle_rad: Real,
    pub launch_direction_rad: Real,
    pub spin_rate_rad_s: Real,
    pub spin_axis_rad: Real,
    pub club_speed_mps: Real,
    pub smash_factor: Real,
    pub attack_angle_rad: Real,
    pub club_path_rad: Real,
    pub face_angle_rad: Real,
    pub face_to_path_rad: Real,
    pub dynamic_loft_rad: Real,
    pub strike_toe_m: Real,
    pub strike_up_m: Real,
    pub best_up_m: Real,
    pub face_half_width_m: Real,
    pub face_height_m: Real,
    pub face_center_height_m: Real,
    pub is_wood: bool,
    pub fat: bool,
    pub power: Real,
    pub target_power: Real,
    pub timing_error_s: Real,
    pub timing_fault: Real,
    pub timed_out: bool,
    pub on_time: bool,
}

/// Observe a hole without changing its prompt, records, or simulation state.
/// This uses the current ball and the address the hole would hand to a player.
pub fn observe(hole: &Hole, mode: Mode) -> Observation {
    let setup = &hole.record().setup;
    let course = hole.course();
    let layout = course.hole(setup.hole).expect("checked hole");
    let world = hole.world_at(hole.clock_s());
    let cup = world.cup_position().expect("checked cup");
    let prompt = hole.prompt();
    let ball = hole.ball_lie();
    let caddie = mode == Mode::Caddie;
    let to_flag = ball.map(|ball| {
        let distance = horizontal(cup - ball.spot).length();
        Distance::new(distance, distance < PUTTING_DISTANCE_M)
    });
    let flag_height = world.ground().sample_ground(cup.x, cup.z).height_m;
    let rise = ball.map(|ball| flag_height - ball.spot.y);
    let to_cup = to_flag
        .zip(rise)
        .map(|(flat, rise)| Distance::new(Vec2::new(flat.exact_m, rise).length(), false));
    let address = hole.advice_in(mode).zip(ball).map(|(advice, ball)| {
        let previous = hole
            .last_address()
            .filter(|facts| facts.club_id == advice.club);
        let restored = previous.filter(|_| prompt.from_last_spot());
        let aim = restored.map_or(advice.first_aim, |facts| facts.aim);
        let fitted = previous.map_or_else(
            || Address::stock(advice.club),
            |facts| facts.address.clone(),
        );
        let club = advice.club;
        let lie = world.address_lie(ball, &fitted.footprint(club), aim);
        let input = StrokeInput::new(
            DeliveryInput {
                club,
                address: fitted,
                lie,
                attempt_seed: Seed(0),
            },
            aim,
        );
        address_reading(
            &world,
            input,
            cup,
            caddie,
            hole.last_address_lie().is_teed(),
        )
    });
    let green = ball
        .filter(|ball| prompt.kind == PromptKind::Play && ball.surface == Surface::Green)
        .map(|ball| GreenReading {
            fall: fall_at(&world, ball.spot.x, ball.spot.z).fall,
            overlay: (caddie || setup.settings.tour_slopes)
                .then(|| green_overlay(&world, &layout.green.outline, setup.green_speed_m))
                .flatten(),
        });
    let relief_available = prompt.kind == PromptKind::Play
        && prompt
            .ruling
            .is_some_and(|ruling| ruling.provisional.is_none());
    let relief_offers = if caddie
        && matches!(
            prompt.kind,
            PromptKind::Play | PromptKind::MaybeLost | PromptKind::Choose
        ) {
        hole.legal()
            .into_iter()
            .filter_map(|action| {
                let Action::Relief { option, .. } = action else {
                    return None;
                };
                let penalty_strokes = match option {
                    ReliefOption::StrokeAndDistance
                    | ReliefOption::BackOnLine
                    | ReliefOption::Lateral
                    | ReliefOption::BunkerAbnormalOutside => 1,
                    ReliefOption::OutsideBunker => 2,
                    _ => 0,
                };
                Some(ReliefOffer {
                    option,
                    penalty_strokes,
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    let aim = address
        .as_ref()
        .map(|address| Vec2::new(address.aim[0], address.aim[2]));
    Observation {
        mode,
        prompt: prompt.kind,
        score: score(
            hole,
            layout.par,
            setup.tee(course).expect("checked tee").length_yd,
        ),
        cup: point(cup),
        ball: ball.map(lie_reading),
        to_flag,
        to_cup,
        flag_rise_m: rise.filter(|_| caddie),
        wind: ball.map(|ball| wind(&world, ball.spot, aim, caddie)),
        address,
        green,
        relief_available,
        relief_offers,
        last_stroke: hole
            .last_stroke()
            .map(|(facts, result)| shot_report(facts, &result, cup)),
    }
}

impl Hole {
    pub fn observe(&self, mode: Mode) -> Observation {
        observe(self, mode)
    }
}

fn point(vec: Vec3) -> Point {
    [vec.x, vec.y, vec.z]
}

fn horizontal(vec: Vec3) -> Vec2 {
    Vec2::new(vec.x, vec.z)
}

fn lie_reading(lie: BallLie) -> LieReading {
    LieReading {
        spot: point(lie.spot),
        surface: lie.surface,
        sit: lie.sit,
        divot: lie.divot,
        ball_height_m: lie.ball_height_m,
        grass_height_m: lie.grass_height_m,
        grass_density: lie.grass_density,
        sand_firmness: lie.sand_firmness,
    }
}

fn wind(world: &World<'_>, spot: Vec3, aim: Option<Vec2>, caddie: bool) -> Wind {
    let reading = world.wind_readout(spot, 0.0);
    let felt = reading.felt_mps;
    let speed = horizontal(felt).length();
    let calm = speed < 0.2;
    let (along_aim, across_aim) = aim.filter(|_| !calm).map_or((None, None), |forward| {
        let direction = horizontal(felt) * (1.0 / speed);
        let along = direction.dot(forward);
        let across = direction.dot(Vec2::new(-forward.y, forward.x));
        (
            (along.abs() > 0.38).then_some(if along > 0.0 {
                AlongWind::Helping
            } else {
                AlongWind::Into
            }),
            (across.abs() > 0.38).then_some(if across > 0.0 {
                CrossWind::FromLeft
            } else {
                CrossWind::FromRight
            }),
        )
    });
    Wind {
        felt_vector_mps: point(felt),
        felt_mps: speed,
        felt_mph: if calm {
            0
        } else {
            (speed / MPH).round() as i64
        },
        from_deg: (!calm).then(|| fm::atan2(-felt.x, felt.z).to_degrees().rem_euclid(360.0)),
        along_aim,
        across_aim,
        flag_mps: point(reading.mean_mps + reading.gust_mps),
        under_trees: reading.under_trees,
        ground_mps: point(reading.ground_mps),
        at_heights_mps: world
            .wind_at_heights(spot, 0.0, &WIND_HEIGHTS_M)
            .into_iter()
            .map(point)
            .collect(),
        temperature_c: caddie.then_some(reading.temperature_c),
        displayed_temperature_c: caddie.then_some(reading.temperature_c.round() as i64),
    }
}

fn address_reading(
    world: &World<'_>,
    mut input: StrokeInput,
    cup: Vec3,
    caddie: bool,
    stock_teed: bool,
) -> AddressReading {
    let lie = input.delivery.lie;
    let ball = lie.ball;
    let club = input.delivery.club;
    let aim = input.aim;
    let putting = club == ClubId::Putter;
    let preview = caddie.then(|| {
        let mut preview = stroke::preview(&input, world, Some(cup), None);
        if !putting {
            preview.putt_line.clear();
        }
        preview_reading(preview)
    });
    // Stock carry follows the club and settings in hand from the previous address.
    input.delivery.lie = AddressLie {
        ball: BallLie::on(
            if stock_teed {
                Surface::Tee
            } else {
                Surface::Fairway
            },
            Sit::Normal,
            ball.spot,
        ),
        slope_along_rad: 0.0,
        slope_across_rad: 0.0,
        ball_slope_along_rad: 0.0,
        ..lie
    };
    let distance = |m| finite(m).map(|m| Distance::new(m, false));
    let stock_carry = distance(stroke::stock_carry(&input));
    let range = horizontal(cup - ball.spot).length();
    let landing = preview
        .as_ref()
        .and_then(|preview| preview.nominal_carry)
        .or(stock_carry)
        .filter(|carry| carry.exact_m < range - LANDING_SHORT_OF_FLAG_M)
        .map(|carry| ball.spot + aim * carry.exact_m)
        .map(|p| {
            point(Vec3::new(
                p.x,
                world.ground().sample_ground(p.x, p.z).height_m,
                p.z,
            ))
        });
    let rounded_slope = |rad: Real| {
        let degrees = rad.to_degrees();
        if degrees.abs() < 0.75 {
            0
        } else {
            degrees.round() as i64
        }
    };
    let spec = standard_club(club);
    AddressReading {
        club,
        club_length_m: spec.length_m,
        club_lie_rad: golf_sim_stroke::tuning::club(club).lie_deg.to_radians(),
        face_half_width_m: spec.face_half_width_m,
        face_height_m: spec.face_height_m,
        face_is_wood: bag::group(club) == Group::Woods,
        putting,
        aim: point(aim),
        aim_point: point(ball.spot + aim * range),
        flagstick_in: input.flagstick_in,
        stock_carry,
        landing,
        preview,
        stance: Stance {
            slope_along_rad: lie.slope_along_rad,
            slope_across_rad: lie.slope_across_rad,
            ball_slope_along_rad: lie.ball_slope_along_rad,
            along_deg: rounded_slope(lie.slope_along_rad),
            across_deg: rounded_slope(lie.slope_across_rad),
            lead_foot_surface: lie.lead_foot_surface,
            trail_foot_surface: lie.trail_foot_surface,
            club_entry_surface: lie.club_entry_surface,
        },
    }
}

fn finite(value: Real) -> Option<Real> {
    value.is_finite().then_some(value)
}

fn preview_reading(preview: Preview) -> PreviewReading {
    let plays_like_m = finite(preview.plays_like_m);
    let carry = finite(preview.carry_m).map(|m| Distance::new(m, false));
    let displayed_plays_like_yd = plays_like_m
        .filter(|m| (m - preview.target_distance_m).abs() >= 0.5 * PLAYS_LIKE_STEP_M)
        .map(|m| (m / PLAYS_LIKE_STEP_M).round() as i64 * 5);
    let nominal_carry = carry.map(|carry| {
        let metres = match (finite(preview.target_distance_m), plays_like_m) {
            (None, _) => carry.exact_m,
            (Some(_), None) => (carry.exact_m / PLAYS_LIKE_STEP_M).round() * PLAYS_LIKE_STEP_M,
            (Some(target), Some(plays)) => {
                let read = displayed_plays_like_yd.map_or(target, |yards| yards as Real * YARD);
                preview.carry_to_target_m * plays / read
            }
        };
        Distance::new(metres, false)
    });
    PreviewReading {
        plays_like_m,
        displayed_plays_like_yd,
        carry,
        carry_to_target_m: finite(preview.carry_to_target_m),
        nominal_carry,
        nominal_roll: finite(preview.roll_m).map(|m| Distance::new(m, true)),
        putt_line: preview.putt_line.into_iter().map(point).collect(),
        aloft: preview.aloft,
        wind_aloft_mps: point(preview.wind_aloft_mps),
    }
}

fn fall_at(world: &World<'_>, x: Real, z: Real) -> GreenSample {
    let ground = world.ground().sample_ground(x, z);
    let n = ground.normal;
    GreenSample {
        point: [x, ground.height_m, z],
        fall: [n.x / n.y, 0.0, n.z / n.y],
    }
}

fn green_overlay(world: &World<'_>, outline: &[Vec2], speed_m: Real) -> Option<GreenOverlay> {
    if outline.len() < 3 {
        return None;
    }
    let extent = |coord: fn(Vec2) -> Real| {
        let values = outline.iter().copied().map(coord);
        (
            values.clone().fold(Real::INFINITY, Real::min),
            values.fold(Real::NEG_INFINITY, Real::max),
        )
    };
    let (left, right) = extent(|p| p.x);
    let (top, bottom) = extent(|p| p.y);
    let mut samples = Vec::new();
    let mut z = top + 0.5;
    while z < bottom {
        let mut x = left + 0.5;
        while x < right {
            if geometry::is_point_in_polygon(Vec2::new(x, z), outline) {
                samples.push(fall_at(world, x, z));
            }
            x += 1.0;
        }
        z += 1.0;
    }
    Some(GreenOverlay { speed_m, samples })
}

fn score(hole: &Hole, par: i64, length_yd: i64) -> Score {
    let rules = hole.rules();
    let strokes = rules.strokes();
    let penalties = rules.penalties();
    let mut number = 0;
    let card = rules
        .rulings
        .iter()
        .filter_map(|ruling| {
            let attempt = ruling.attempt? as i64;
            let facts = hole.record().entries.iter().find_map(|entry| match entry {
                Entry::Stroke(stroke) if stroke.facts.attempt == attempt => Some(&stroke.facts),
                _ => None,
            })?;
            number += 1;
            let row = CardStroke {
                number,
                club: facts.club_id,
                from: point(ruling.from),
                rest: point(ruling.rest),
                distance: Distance::new(horizontal(ruling.rest - ruling.from).length(), false),
                rest_surface: facts.lie.surface,
                status: ruling.status,
                penalties: ruling.penalty_strokes,
            };
            number += ruling.penalty_strokes;
            Some(row)
        })
        .collect();
    Score {
        number: hole.record().setup.hole,
        par,
        length_yd,
        strokes,
        next_stroke: (hole.prompt().kind != PromptKind::Done).then_some(strokes + 1),
        penalties,
        to_par: strokes as i64 - par,
        provisional: rules
            .last()
            .is_some_and(|ruling| ruling.provisional.is_some()),
        card,
    }
}

fn shot_report(facts: &StrokeFacts, result: &StrokeResult, cup: Vec3) -> ShotReport {
    let shot = &result.playback;
    let launch = shot.launch.expect("a played stroke records its launch");
    let execution = facts.execution;
    let target_power = facts.address.swing_length;
    let club = standard_club(facts.club_id);
    let putting = shot.carry_m() == 0.0;
    let on_time = !execution.timed_out && execution.timing_fault.abs() <= 0.35;
    let power_points = (execution.power * 100.0).round() - (target_power * 100.0).round();
    let cause = if execution.timed_out {
        ShotCause::Timing
    } else if result.fat || !matches!(launch.contact, Contact::Face | Contact::Sand) {
        ShotCause::Strike
    } else if execution.timing_fault.abs() > 1.0 {
        ShotCause::Timing
    } else if result.quality == StrikeQuality::Mishit {
        ShotCause::Strike
    } else if power_points.abs() >= 5.0 {
        ShotCause::Power
    } else if on_time {
        ShotCause::None
    } else {
        ShotCause::Timing
    };
    let proximity = horizontal(shot.rest_position - cup).length();
    ShotReport {
        club: facts.club_id,
        end: result.outcome.end,
        quality: result.quality,
        contact: launch.contact,
        cause,
        carry: Distance::new(shot.carry_m(), false),
        total: Distance::new(shot.total_m(), putting),
        offline: Distance::new(shot.carry_side_m(), false),
        total_offline: Distance::new(shot.total_side_m(), false),
        proximity: Distance::new(proximity, proximity < PUTTING_DISTANCE_M),
        apex_m: shot.apex_height_m(),
        hang_time_s: shot.hang_time_s(),
        land_angle_rad: shot.land_angle_rad(),
        ball_speed_mps: launch.readout.ball_speed_mps,
        launch_angle_rad: launch.readout.launch_angle_rad,
        launch_direction_rad: launch.readout.launch_direction_rad,
        spin_rate_rad_s: launch.readout.spin_rate_rad_s,
        spin_axis_rad: launch.readout.spin_axis_rad,
        club_speed_mps: launch.club_speed_mps,
        smash_factor: launch.smash_factor,
        attack_angle_rad: launch.attack_angle_rad,
        club_path_rad: launch.club_path_rad,
        face_angle_rad: launch.face_angle_rad,
        face_to_path_rad: launch.face_to_path_rad,
        dynamic_loft_rad: launch.dynamic_loft_rad,
        strike_toe_m: launch.strike_toe_m,
        strike_up_m: launch.strike_up_m,
        best_up_m: golf_sim_stroke::tuning::club(facts.club_id).strike_up_mm / 1000.0,
        face_half_width_m: club.face_half_width_m,
        face_height_m: club.face_height_m,
        face_center_height_m: club.face_center_height_m,
        is_wood: bag::group(facts.club_id) == Group::Woods,
        fat: result.fat,
        power: execution.power,
        target_power,
        timing_error_s: execution.timing_error_s,
        timing_fault: execution.timing_fault,
        timed_out: execution.timed_out,
        on_time,
    }
}
