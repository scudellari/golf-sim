use std::cell::Cell;

use golf_sim_base::{
    ClubId, Real, Seed, Surface, Vec2, Vec3,
    digest::object,
    fm,
    trace::{NoProbe, Probe},
    variant::{Array, Components, Dict, FloatWidth, Variant},
};
use golf_sim_conditions::{
    lies::{AddressLie, BallLie, DivotLie, Sit},
    world::World,
};
use golf_sim_course::course::Course;
use golf_sim_physics::{
    facade as sim,
    integrator::{self, Checkpoint, Options},
    plane::ConstantAir,
    samplers::{AirSample, AirSampler, GroundSampler},
    values::{Launch, ShotRequest, ShotResult},
};
use golf_sim_stroke::{
    address::{Address, Assists},
    delivery::{self, DeliveryInput},
    results::{DropResult, Preview, StrokeResult},
    stroke::{self, StrokeInput},
};
use golf_sim_swing::{
    execution::Execution,
    spec::{StrikeMode, SwingSpec},
};

const ABSENT: Vec3 = Vec3::new(Real::INFINITY, Real::INFINITY, Real::INFINITY);

macro_rules! record {
    ($($key:ident: $value:expr),+ $(,)?) => {
        object([$( (stringify!($key), ($value).into()) ),+])
    };
}

pub(super) fn field<'a>(input: &'a Dict, name: &str) -> &'a Variant {
    input.get(name).unwrap_or_else(|| panic!("missing {name}"))
}

pub(super) fn dict(value: &Variant) -> &Dict {
    value.as_dict().unwrap()
}

fn items(value: &Variant) -> &[Variant] {
    let Variant::Array(array) = value else {
        panic!("expected an array");
    };
    &array.items
}

fn integer(value: &Variant) -> i64 {
    let Variant::Int(number) = value else {
        panic!("expected an integer");
    };
    number.value()
}

fn code(value: &Variant) -> usize {
    usize::try_from(integer(value)).unwrap()
}

pub(super) fn number(input: &Dict, name: &str) -> Real {
    match field(input, name) {
        Variant::Int(number) => number.value() as Real,
        Variant::Float(number) => number.value(),
        _ => panic!("expected a number at {name}"),
    }
}

fn boolean(value: &Variant) -> bool {
    let Variant::Bool(value) = value else {
        panic!("expected a boolean");
    };
    *value
}

fn vector2(value: &Variant) -> Vec2 {
    let Variant::Vector2(vector) = value else {
        panic!("expected a Vector2");
    };
    let [x, y] = vector.values;
    Vec2::new(x, y)
}

pub(super) fn vector3(value: &Variant) -> Vec3 {
    let Variant::Vector3(vector) = value else {
        panic!("expected a Vector3");
    };
    let [x, y, z] = vector.values;
    Vec3::new(x, y, z)
}

pub(super) fn vector(point: Vec3) -> Variant {
    Variant::Vector3(Components {
        values: [point.x, point.y, point.z],
        width: FloatWidth::Double,
    })
}

fn array(values: impl IntoIterator<Item = Variant>) -> Variant {
    Variant::Array(Array::new(values.into_iter().collect()))
}

fn ball_lie(input: &Dict) -> BallLie {
    BallLie {
        spot: vector3(field(input, "spot")),
        surface: Surface::ALL[code(field(input, "surface"))],
        sit: [Sit::Up, Sit::Normal, Sit::Down][code(field(input, "sit"))],
        ball_height_m: number(input, "ball_height_m"),
        grass_height_m: number(input, "grass_height_m"),
        grass_density: number(input, "grass_density"),
        sand_firmness: number(input, "sand_firmness"),
        divot: [
            DivotLie::None,
            DivotLie::Fresh,
            DivotLie::Old,
            DivotLie::SandFilled,
        ][code(field(input, "divot"))],
    }
}

fn playback_snapshot(result: &ShotResult) -> Variant {
    record! {
        launch: result.launch.map_or(Variant::Nil, launch_snapshot),
        times: Variant::PackedFloat64Array(result.times.clone()),
        positions: packed_points(&result.positions),
        events: array(result.events.iter().map(|event| record! {
            kind: event.kind as i64,
            time_s: event.time_s,
            position: vector(event.position),
            velocity: vector(event.velocity),
            spin: vector(event.spin),
            surface: event.surface as i64,
            zone: event.zone as i64,
        })),
        touchdowns: array(result.touchdowns.iter().map(|touchdown| record! {
            time_s: touchdown.time_s,
            position: vector(touchdown.position),
            contact_point: vector(touchdown.contact_point),
            velocity: vector(touchdown.velocity),
            depth_m: touchdown.depth_m,
            surface: touchdown.surface as i64,
        })),
        end_reason: result.end_reason as i64,
        rest_position: vector(result.rest_position),
        rest_surface: result.rest_surface as i64,
        embedded: Variant::Bool(result.embedded),
    }
}

pub(super) fn launch_snapshot(launch: Launch) -> Variant {
    let readout = launch.readout;
    record! {
        position: vector(launch.position),
        velocity: vector(launch.velocity),
        spin: vector(launch.spin),
        aim: vector(launch.aim),
        ball_speed_mps: readout.ball_speed_mps,
        launch_angle_rad: readout.launch_angle_rad,
        launch_direction_rad: readout.launch_direction_rad,
        spin_rate_rad_s: readout.spin_rate_rad_s,
        spin_axis_rad: readout.spin_axis_rad,
        club_speed_mps: launch.club_speed_mps,
        club_speed_at_ball_mps: launch.club_speed_at_ball_mps,
        smash_factor: launch.smash_factor,
        club_path_rad: launch.club_path_rad,
        attack_angle_rad: launch.attack_angle_rad,
        face_angle_rad: launch.face_angle_rad,
        dynamic_loft_rad: launch.dynamic_loft_rad,
        face_to_path_rad: launch.face_to_path_rad,
        spin_loft_rad: launch.spin_loft_rad,
        valid: Variant::Bool(launch.valid),
        contact: launch.contact as i64,
        strike_toe_m: launch.strike_toe_m,
        strike_up_m: launch.strike_up_m,
        low_point_ahead_m: launch.low_point_ahead_m,
        low_point_height_m: launch.low_point_height_m,
        turf_entry_behind_m: launch.turf_entry_behind_m,
        turf_contact_start: vector(launch.turf_contact_start.unwrap_or(ABSENT)),
        turf_contact_end: vector(launch.turf_contact_end.unwrap_or(ABSENT)),
        turf_contact_depth_m: launch.turf_contact_depth_m,
        interference: launch.interference,
        ball_temperature_c: launch.ball_temperature_c,
    }
}

pub(super) fn stroke_input(input: &Dict) -> StrokeInput {
    let lie = dict(field(input, "lie"));
    let address = dict(field(input, "address"));
    StrokeInput {
        delivery: DeliveryInput {
            club: ClubId::from_name(field(input, "club_id").as_str().unwrap()).unwrap(),
            address: Address::from_dict(address),
            lie: AddressLie {
                ball: ball_lie(dict(field(lie, "ball"))),
                slope_along_rad: number(lie, "slope_along_rad"),
                slope_across_rad: number(lie, "slope_across_rad"),
                ball_slope_along_rad: number(lie, "ball_slope_along_rad"),
                lead_foot_surface: Surface::ALL[code(field(lie, "lead_foot_surface"))],
                trail_foot_surface: Surface::ALL[code(field(lie, "trail_foot_surface"))],
                club_entry_surface: Surface::ALL[code(field(lie, "club_entry_surface"))],
            },
            attempt_seed: Seed(integer(field(input, "attempt_seed"))),
        },
        aim: vector3(field(input, "aim")),
        gust_time_s: number(input, "gust_time_s"),
        flagstick_in: boolean(field(input, "flagstick_in")),
    }
}

fn execution(input: &Dict) -> Execution {
    Execution {
        power: number(input, "power"),
        timing_fault: number(input, "timing_fault"),
        timed_out: boolean(field(input, "timed_out")),
        strike_m: boolean(field(input, "has_strike")).then(|| vector2(field(input, "strike_m"))),
        timing_error_s: 0.0,
        impact_usec: 0,
        backswing_s: 0.0,
        ring_s: 0.0,
    }
}

fn packed_points(points: &[Vec3]) -> Variant {
    Variant::PackedVector3Array(Components {
        values: points.iter().map(|p| [p.x, p.y, p.z]).collect(),
        width: FloatWidth::Double,
    })
}

fn pair(point: Vec2) -> Variant {
    Variant::Vector2(Components {
        values: [point.x, point.y],
        width: FloatWidth::Double,
    })
}

fn swing_snapshot(swing: SwingSpec) -> Variant {
    record! {
        target_power: swing.target_power,
        power_cap: swing.power_cap,
        meter_rate_per_s: swing.meter_rate_per_s,
        tempo_ratio: swing.tempo_ratio,
        ring_min_s: swing.ring_min_s,
        ring_max_s: swing.ring_max_s,
        window_early_s: swing.window_early_s,
        window_late_s: swing.window_late_s,
        timeout_after_s: swing.timeout_after_s,
        strike_mode: swing.strike_mode as i64,
        drift_bias_m: pair(swing.drift_bias_m),
        drift_start_sd_m: pair(swing.drift_start_sd_m),
        drift_slide_sd_mps: pair(swing.drift_slide_sd_mps),
        steer_vertical: Variant::Bool(swing.steer_vertical),
        downswing_slide_gain: swing.downswing_slide_gain,
        wobble_m: pair(swing.wobble_m),
    }
}

fn stroke_snapshot(result: &StrokeResult) -> Variant {
    let outcome = &result.outcome;
    record! {
        playback: playback_snapshot(&result.playback),
        outcome: record! {
            end: outcome.end as i64,
            start: vector(outcome.start),
            rest: vector(outcome.rest),
            rest_surface: outcome.rest_surface as i64,
            ball_radius_m: outcome.ball_radius_m,
            carry_m: outcome.carry_m,
            total_m: outcome.total_m,
            embedded: Variant::Bool(outcome.embedded),
            penalty_entry: vector(outcome.penalty_entry.unwrap_or(ABSENT)),
            marks: array(outcome.marks.iter().map(|mark| record! {
                kind: mark.kind as i64,
                start: vector(mark.start),
                end: vector(mark.end),
                depth_m: mark.depth_m,
                surface: mark.surface as i64,
            })),
        },
        fault: result.fault,
        quality: result.quality as i64,
        fat: Variant::Bool(result.fat),
    }
}

fn preview_snapshot(preview: &Preview) -> Variant {
    record! {
        carry_m: preview.carry_m,
        target_distance_m: preview.target_distance_m,
        carry_to_target_m: preview.carry_to_target_m,
        plays_like_m: preview.plays_like_m,
        wind_aloft_mps: vector(preview.wind_aloft_mps),
        wind_here_mps: vector(preview.wind_here_mps),
        aloft: preview.aloft as i64,
        roll_m: preview.roll_m,
        putt_line: packed_points(&preview.putt_line),
        has_hint: Variant::Bool(preview.has_hint),
        hint_carry_m: preview.hint_carry_m,
        hint_apex_m: preview.hint_apex_m,
        hint_offline_m: preview.hint_offline_m,
    }
}

fn lie_snapshot(lie: BallLie) -> Variant {
    record! {
        spot: vector(lie.spot),
        surface: lie.surface as i64,
        sit: lie.sit as i64,
        ball_height_m: lie.ball_height_m,
        grass_height_m: lie.grass_height_m,
        grass_density: lie.grass_density,
        sand_firmness: lie.sand_firmness,
        divot: lie.divot as i64,
    }
}

fn replay(case: &Dict, world: &World<'_>, probe: &mut impl Probe) -> Variant {
    let args = dict(field(case, "input"));
    let call = field(case, "call").as_str().unwrap();
    match call {
        "stroke.drop" => {
            let DropResult { first, rest } = stroke::drop(
                vector3(field(args, "spot")),
                number(args, "knee_height_m"),
                world,
                &mut NoProbe,
            );
            record! { first: vector(first.unwrap_or(ABSENT)), rest: vector(rest.unwrap_or(ABSENT)) }
        }
        "stroke.place" => lie_snapshot(stroke::place(
            vector3(field(args, "spot")),
            world,
            Seed(integer(field(args, "seed_value"))),
            &mut NoProbe,
        )),
        _ => {
            let input = stroke_input(dict(field(args, "input")));
            let execution = || execution(dict(field(args, "execution")));
            match call {
                "stroke.swing_spec" => {
                    let assists = dict(field(args, "assists"));
                    swing_snapshot(stroke::swing_spec(
                        &input,
                        Assists {
                            window_scale: number(assists, "window_scale"),
                            strike_mode: [StrikeMode::Reticle, StrikeMode::None]
                                [code(field(assists, "strike_mode"))],
                        },
                    ))
                }
                "stroke.launch" => {
                    launch_snapshot(stroke::launch(&input, execution(), world, &mut NoProbe))
                }
                "stroke.play" => stroke_snapshot(&stroke::play(&input, execution(), world, probe)),
                "stroke.preview" => {
                    let target = vector3(field(args, "target"));
                    let stock = field(args, "stock_address")
                        .as_dict()
                        .map(Address::from_dict);
                    preview_snapshot(&stroke::preview(
                        &input,
                        world,
                        Some(target),
                        stock.as_ref(),
                    ))
                }
                _ => panic!("unknown stroke call {call}"),
            }
        }
    }
}

const SINGLE_STEP: Real = 1.0 / 8_388_608.0;

pub(super) fn fields_mut(value: &mut Variant) -> &mut Dict {
    let Variant::Dict(dict) = value else {
        panic!("expected a dictionary");
    };
    dict
}

pub(super) fn magnitude(point: Vec3) -> Real {
    point.x.abs().max(point.y.abs()).max(point.z.abs())
}

pub(super) fn horizontal(point: Vec3) -> Vec2 {
    Vec2::new(point.x, point.z)
}

fn exceeds_default(a: Real, b: Real, scale: Real) -> bool {
    let bound = 1e-6_f64
        .max(1e-5 * a.abs().max(b.abs()))
        .max(8.0 * SINGLE_STEP * scale.max(1.0));
    let error = (a - b).abs();
    error.is_finite() && error > bound
}

pub(super) struct DerivedChecks<'a> {
    pub(super) id: &'a str,
    pub(super) drift: bool,
    pub(super) failure: Option<String>,
}

impl DerivedChecks<'_> {
    fn number(
        &mut self,
        expected: &Dict,
        actual: &mut Dict,
        key: &str,
        reason: &str,
        allowed: impl FnOnce(Real, Real) -> Real,
    ) {
        self.try_number(expected, actual, key, reason, |a, b| Ok(allowed(a, b)));
    }

    fn try_number(
        &mut self,
        expected: &Dict,
        actual: &mut Dict,
        key: &str,
        reason: &str,
        allowed: impl FnOnce(Real, Real) -> Result<Real, String>,
    ) {
        let a = number(expected, key);
        let b = number(actual, key);
        let error = (a - b).abs();
        if exceeds_default(a, b, a.abs().max(b.abs())) {
            let bound = match allowed(a, b) {
                Ok(bound) => bound,
                Err(message) => {
                    self.failure
                        .get_or_insert_with(|| format!("{key}: {message}"));
                    return;
                }
            };
            if !(self.drift || error <= bound) {
                self.failure.get_or_insert_with(|| {
                    format!("{key}: {error:.6e} exceeds {bound:.6e} ({reason})")
                });
                return;
            }
            let relation = if error <= bound { "≤" } else { ">" };
            println!(
                "{} {key}: {error:.6e} {relation} {bound:.6e} ({reason})",
                self.id
            );
            if !self.drift {
                *actual.get_mut(key).unwrap() = field(expected, key).clone();
            }
        }
    }

    pub(super) fn launch(&mut self, expected: &Dict, actual: &mut Dict, standalone: bool) {
        let a = vector3(field(expected, "velocity"));
        let b = vector3(field(actual, "velocity"));
        let angular_change = fm::atan2(a.cross(b).length(), a.dot(b));
        self.number(
            expected,
            actual,
            "launch_angle_rad",
            "direction of the recorded velocity",
            |_, _| angular_change + 8.0 * SINGLE_STEP,
        );
        self.number(
            expected,
            actual,
            "spin_loft_rad",
            "acos of a rounded unit-vector dot product",
            |_, angle| {
                let dot = fm::cos(angle);
                let rounding = 8.0 * SINGLE_STEP;
                (fm::acos((dot - rounding).max(-1.0)) - angle)
                    .max(angle - fm::acos((dot + rounding).min(1.0)))
            },
        );
        let speed_error = (a - b).length() + 8.0 * SINGLE_STEP * a.length().max(1.0);
        if standalone {
            self.number(
                expected,
                actual,
                "ball_speed_mps",
                "length of the checked velocity",
                |_, _| speed_error,
            );
        }
        let ac = number(expected, "club_speed_mps");
        let bc = number(actual, "club_speed_mps");
        let quotient_error = speed_error / ac + (ac - bc).abs() * b.length() / (ac * bc);
        self.number(
            expected,
            actual,
            "smash_factor",
            "ratio of the checked ball and club speeds",
            |_, _| quotient_error,
        );
    }

    fn repeated_point(
        &mut self,
        expected: &Dict,
        actual: &mut Dict,
        key: &str,
        source: [Vec3; 2],
    ) -> bool {
        let a = vector3(field(expected, key));
        let b = vector3(field(actual, key));
        if [a, b] != source {
            self.failure.get_or_insert_with(|| {
                format!(
                    "{key}: repeated point differs from its source: {:?} != {source:?}",
                    [a, b]
                )
            });
            return false;
        }
        let scale = magnitude(a).max(magnitude(b));
        let needs_check = [(a.x, b.x), (a.y, b.y), (a.z, b.z)]
            .into_iter()
            .any(|(a, b)| exceeds_default(a, b, scale));
        if needs_check && !self.drift {
            *actual.get_mut(key).unwrap() = field(expected, key).clone();
        }
        needs_check
    }

    fn outcome(
        &mut self,
        expected: &Dict,
        actual: &mut Dict,
        expected_shot: &Dict,
        actual_shot: &Dict,
    ) {
        let a = vector3(field(expected_shot, "rest_position"));
        let b = vector3(field(actual_shot, "rest_position"));
        if self.repeated_point(expected, actual, "rest", [a, b]) {
            println!("{} outcome.rest repeats the checked playback rest", self.id);
        }
        let start = |shot: &Dict| vector3(field(dict(field(shot, "launch")), "position"));
        let a0 = start(expected_shot);
        let b0 = start(actual_shot);
        self.number(
            expected,
            actual,
            "total_m",
            "difference of the checked rest and launch points",
            |_, _| {
                horizontal((a - b) - (a0 - b0)).length()
                    + 8.0 * SINGLE_STEP * magnitude(a).max(magnitude(a0)).max(1.0)
            },
        );

        let expected_marks = items(field(expected, "marks"));
        let Some(Variant::Array(actual_marks)) = actual.get_mut("marks") else {
            panic!("expected wear marks");
        };
        let touches = |shot: &Dict| {
            items(field(shot, "touchdowns"))
                .iter()
                .map(dict)
                .filter(|touch| number(touch, "depth_m") > 0.0)
                .map(|touch| vector3(field(touch, "contact_point")))
                .collect::<Vec<_>>()
        };
        let mut checked_marks = false;
        let mut expected_touches = touches(expected_shot).into_iter();
        let mut actual_touches = touches(actual_shot).into_iter();
        for (a, b) in expected_marks.iter().zip(&mut actual_marks.items) {
            let a = dict(a);
            let b = fields_mut(b);
            if integer(field(a, "kind")) == 1 && integer(field(b, "kind")) == 1 {
                let (Some(expected_contact), Some(actual_contact)) =
                    (expected_touches.next(), actual_touches.next())
                else {
                    self.failure
                        .get_or_insert_with(|| "pitch mark has no touchdown contact".into());
                    break;
                };
                for key in ["start", "end"] {
                    checked_marks |=
                        self.repeated_point(a, b, key, [expected_contact, actual_contact]);
                }
            }
        }
        if checked_marks {
            println!(
                "{} pitch-mark endpoints repeat the checked touchdown contacts",
                self.id
            );
        }
    }
}

struct FlightHeightRounding<'a> {
    air: &'a ConstantAir,
    previous_height: Cell<Option<Real>>,
    step_error: Cell<Real>,
    height_error: Cell<Real>,
}

impl AirSampler for FlightHeightRounding<'_> {
    fn sample_air(&self, time_s: Real, position: Vec3) -> AirSample {
        if let Some(previous) = self.previous_height.replace(Some(position.y)) {
            // Velocity rounding changes the displacement of the steps that follow.
            self.step_error
                .set(self.step_error.get() + SINGLE_STEP * (position.y - previous).abs());
        }
        self.height_error.set(
            self.height_error.get()
                + SINGLE_STEP * position.y.abs().max(1.0)
                + self.step_error.get(),
        );
        self.air.sample_air(time_s, position)
    }
}

fn height_crossing_bound(
    input: &StrokeInput,
    world: &World<'_>,
    height: Real,
    carry: Real,
) -> Result<Real, String> {
    let ground = world.ground();
    let air = world.air_mean(input.ball_position());
    let launch = stroke::nominal_shot(input, world)
        .launch
        .ok_or("nominal shot has no launch")?;
    let ball = sim::standard_ball();
    let rounding = FlightHeightRounding {
        air: &air,
        previous_height: Cell::new(None),
        step_error: Cell::new(0.0),
        height_error: Cell::new(0.0),
    };
    let crossing = |height, air: &dyn AirSampler| {
        sim::carry_to_height(&launch, &ball, world.clock_s(), air, ground, height)
    };
    let replayed = crossing(height, &rounding);
    if replayed != carry {
        return Err(format!(
            "preview carry {carry:.9} m differs from its independent replay {replayed:.9} m"
        ));
    }
    // Each flight step stores a new height in a vector. The crossing distance depends
    // on the descent slope, so measure its sensitivity by moving the crossing plane.
    let dy = rounding.height_error.get() + SINGLE_STEP * height.abs().max(1.0);
    Ok((crossing(height - dy, &air) - carry)
        .abs()
        .max((crossing(height + dy, &air) - carry).abs()))
}

// Independently check the computation-specific bounds before projecting those values
// to the fixture. All other values, including the operands, keep their table bounds.
pub(super) fn impact_request(
    input: &StrokeInput,
    execution: Execution,
    with_noise: bool,
) -> ShotRequest {
    let built = delivery::build(&input.delivery, execution, with_noise, &mut NoProbe);
    let mut request = ShotRequest::new(built.club, sim::standard_ball(), built.delivery, built.lie);
    request.position = input.ball_position();
    request.aim = input.aim;
    request
}

pub(in crate::corpus) fn verify_delivery(
    case: &Dict,
    actual: &Variant,
    course: Option<&Course>,
    limits: &crate::tolerance::Layer,
) -> Result<(), String> {
    use crate::corpus::physics::impact::{compare_launch_rounding, verify_strike_rounding};

    let args = dict(field(case, "input"));
    let world = crate::corpus::world::world(dict(field(args, "world")), course);
    let input = stroke_input(dict(field(args, "input")));
    let request = impact_request(&input, execution(dict(field(args, "execution"))), true);
    fn launch(value: &Variant) -> &Variant {
        field(dict(field(dict(value), "playback")), "launch")
    }
    let expected = field(case, "output");
    let reference_launch = launch(expected);
    let mut computed = dict(launch(actual)).clone();
    verify_strike_rounding(dict(reference_launch), &computed, &request, world.ground())?;
    compare_launch_rounding(
        case,
        dict(reference_launch),
        &mut computed,
        &request,
        world.ground(),
        false,
    )?;
    let mut checks = DerivedChecks {
        id: field(case, "id").as_str().unwrap(),
        drift: false,
        failure: None,
    };
    checks.launch(dict(reference_launch), &mut computed, false);
    if let Some(failure) = checks.failure {
        return Err(failure);
    }
    if let Some(field) = limits.checkpoint_difference(
        reference_launch,
        &Variant::Dict(computed),
        "playback.launch",
    ) {
        return Err(format!("playback.launch.{field} exceeds output row"));
    }
    for key in ["fault", "quality", "fat"] {
        if limits
            .checkpoint_difference(field(dict(expected), key), field(dict(actual), key), key)
            .is_some()
        {
            return Err(format!("{key} exceeds output row"));
        }
    }
    Ok(())
}

fn check_derived_fields(
    case: &Dict,
    world: &World<'_>,
    actual: &mut Variant,
    drift: bool,
    model_departure: bool,
) -> Result<(), String> {
    let mut checks = DerivedChecks {
        id: field(case, "id").as_str().unwrap(),
        drift,
        failure: None,
    };
    let expected = dict(field(case, "output"));
    let actual = fields_mut(actual);
    match field(case, "call").as_str().unwrap() {
        "stroke.launch" => {
            let args = dict(field(case, "input"));
            let input = stroke_input(dict(field(args, "input")));
            let request = impact_request(&input, execution(dict(field(args, "execution"))), true);
            crate::corpus::physics::impact::compare_launch_rounding(
                case,
                expected,
                actual,
                &request,
                world.ground(),
                drift,
            )?;
            checks.launch(expected, actual, true);
        }
        "stroke.play" => {
            let expected_shot = dict(field(expected, "playback"));
            if !model_departure {
                let mut outcome =
                    std::mem::replace(actual.get_mut("outcome").unwrap(), Variant::Nil);
                checks.outcome(
                    dict(field(expected, "outcome")),
                    fields_mut(&mut outcome),
                    expected_shot,
                    dict(field(actual, "playback")),
                );
                *actual.get_mut("outcome").unwrap() = outcome;
            }
            let args = dict(field(case, "input"));
            let input = stroke_input(dict(field(args, "input")));
            let request = impact_request(&input, execution(dict(field(args, "execution"))), true);
            let playback = fields_mut(actual.get_mut("playback").unwrap());
            crate::corpus::physics::impact::compare_shot_launch(
                expected_shot,
                playback,
                |expected, actual| {
                    crate::corpus::physics::impact::compare_launch_rounding(
                        case,
                        expected,
                        actual,
                        &request,
                        world.ground(),
                        drift,
                    )?;
                    checks.launch(expected, actual, false);
                    Ok(())
                },
            )?;
        }
        "stroke.preview" => {
            let args = dict(field(case, "input"));
            let input = stroke_input(dict(field(args, "input")));
            let target = vector3(field(args, "target"));
            for (key, height) in [
                ("carry_m", input.ball_position().y),
                (
                    "carry_to_target_m",
                    if target.is_finite() {
                        world.ground().sample_ground(target.x, target.z).height_m
                    } else {
                        Real::NAN
                    },
                ),
            ] {
                checks.try_number(
                    expected,
                    actual,
                    key,
                    "descent crossing after accumulated height rounding",
                    |_, carry| height_crossing_bound(&input, world, height, carry),
                );
            }
        }
        _ => {}
    }
    checks.failure.map_or(Ok(()), Err)
}

pub(in crate::corpus) fn run(
    case: &Dict,
    course: Option<&Course>,
    drift: bool,
) -> Result<Variant, String> {
    let args = dict(field(case, "input"));
    let world = crate::corpus::world::world(dict(field(args, "world")), course);
    let mut actual = replay(case, &world, &mut NoProbe);
    check_derived_fields(case, &world, &mut actual, drift, false)?;
    Ok(actual)
}

pub(in crate::corpus) fn run_with_model(
    case: &Dict,
    course: Option<&Course>,
    drift: bool,
    versions: crate::corpus::physics::models::Versions,
    limits: &crate::tolerance::Layer,
) -> Result<crate::tolerance::CaseOutcome, String> {
    use crate::tolerance::CaseOutcome;

    let call = field(case, "call").as_str();
    let has_operands =
        call == Some("stroke.preview") && dict(field(case, "output")).get("operands").is_some();
    if call != Some("stroke.play") && !has_operands {
        return run(case, course, drift).map(Into::into);
    }
    let args = dict(field(case, "input"));
    let world = crate::corpus::world::world(dict(field(args, "world")), course);
    if has_operands {
        let actual = replay(case, &world, &mut NoProbe);
        return super::preview::compare(case, &world, limits, actual, drift).map(Into::into);
    }
    let mut outcome = versions.run(|probe| Ok(replay(case, &world, probe)))?;
    match &mut outcome {
        CaseOutcome::Compared { actual, .. } => {
            check_derived_fields(case, &world, actual, drift, false)?;
        }
        CaseOutcome::ModelDeparture { actual, .. } => {
            check_derived_fields(case, &world, actual, drift, true)?;
        }
        _ => unreachable!("a replay produces a comparison"),
    }
    Ok(outcome)
}

pub(in crate::corpus) fn replay_from_launch(
    case: &Dict,
    course: Option<&Course>,
    versions: crate::corpus::physics::models::Versions,
    launch: &Launch,
    checkpoint: Option<Checkpoint>,
) -> Result<crate::tolerance::CaseOutcome, String> {
    use crate::tolerance::CaseOutcome;

    let args = dict(field(case, "input"));
    let world = crate::corpus::world::world(dict(field(args, "world")), course);
    let input = stroke_input(dict(field(args, "input")));
    let execution = execution(dict(field(args, "execution")));
    let mut outcome = versions.run(|probe| {
        let result = stroke::play_with_simulation(
            &input,
            execution,
            &world,
            probe,
            |request, air, ground, probe| match checkpoint {
                Some(checkpoint) => integrator::run_with_checkpoint(
                    launch,
                    &request.ball,
                    air,
                    ground,
                    request.cup,
                    Options {
                        time_s: request.time_s,
                        ..Options::default()
                    },
                    checkpoint,
                ),
                None => Some(sim::simulate_launch(
                    launch,
                    &request.ball,
                    request.time_s,
                    air,
                    ground,
                    request.cup,
                    true,
                    probe,
                )),
            },
        )
        .ok_or("checkpoint unavailable in the core replay")?;
        Ok(stroke_snapshot(&result))
    })?;
    if let CaseOutcome::Compared { actual, .. } = &mut outcome {
        let mut checks = DerivedChecks {
            id: field(case, "id").as_str().unwrap(),
            drift: false,
            failure: None,
        };
        let expected = dict(field(case, "output"));
        let actual = fields_mut(actual);
        let mut interpreted = std::mem::replace(actual.get_mut("outcome").unwrap(), Variant::Nil);
        checks.outcome(
            dict(field(expected, "outcome")),
            fields_mut(&mut interpreted),
            dict(field(expected, "playback")),
            dict(field(actual, "playback")),
        );
        *actual.get_mut("outcome").unwrap() = interpreted;
        if let Some(failure) = checks.failure {
            return Err(failure);
        }
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mismatched_repeated_point_is_a_case_failure() {
        let expected = record!(rest: vector(Vec3::ZERO));
        let mut actual = record!(rest: vector(Vec3::new(1.0, 0.0, 0.0)));
        for drift in [false, true] {
            let mut checks = DerivedChecks {
                id: "synthetic",
                drift,
                failure: None,
            };
            assert!(!checks.repeated_point(
                dict(&expected),
                fields_mut(&mut actual),
                "rest",
                [Vec3::ZERO; 2],
            ));
            assert!(
                checks
                    .failure
                    .unwrap()
                    .contains("rest: repeated point differs")
            );
        }
    }

    #[test]
    fn legacy_preview_carry_must_match_its_independent_replay() {
        let ball = BallLie::on(Surface::Fairway, Sit::Normal, Vec3::ZERO);
        let world = World::practice(
            golf_sim_physics::plane::PlaneGround::default(),
            ConstantAir::new(sim::standard_air()),
            ball,
            None,
            0.0,
        );
        let aim = Vec3::new(0.0, 0.0, -1.0);
        let club = ClubId::Driver;
        let address = stroke::stock(club);
        let lie = world.address_lie(ball, &stroke::footprint(club, &address), aim);
        let input = StrokeInput::new(
            DeliveryInput {
                club,
                address,
                lie,
                attempt_seed: Seed(1),
            },
            aim,
        );
        let carry = stroke::preview(&input, &world, None, None).carry_m;
        assert!(height_crossing_bound(&input, &world, 0.0, carry).is_ok());
        let expected = record!(carry_m: carry);
        let mut actual = record!(carry_m: carry + 1.0);
        let mut checks = DerivedChecks {
            id: "synthetic",
            drift: false,
            failure: None,
        };
        checks.try_number(
            dict(&expected),
            fields_mut(&mut actual),
            "carry_m",
            "descent crossing",
            |_, carry| height_crossing_bound(&input, &world, 0.0, carry),
        );
        assert!(
            checks
                .failure
                .unwrap()
                .starts_with("carry_m: preview carry")
        );
    }

    #[test]
    fn derived_bound_failure_retains_the_computed_value_and_reason() {
        let expected = record!(carry_m: 2.0);
        let mut actual = record!(carry_m: 3.0);
        let mut checks = DerivedChecks {
            id: "synthetic",
            drift: false,
            failure: None,
        };
        checks.number(
            dict(&expected),
            fields_mut(&mut actual),
            "carry_m",
            "descent crossing",
            |_, _| 0.25,
        );
        assert_eq!(number(dict(&actual), "carry_m"), 3.0);
        assert_eq!(
            checks.failure.as_deref(),
            Some("carry_m: 1.000000e0 exceeds 2.500000e-1 (descent crossing)")
        );
    }
}
