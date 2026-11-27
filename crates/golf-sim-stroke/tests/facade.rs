use std::{
    cell::Cell,
    convert::Infallible,
    path::{Path, PathBuf},
};

use golf_sim_base::{
    ClubId, Real, Seed, Surface, Vec2, Vec3,
    digest::object,
    fm,
    trace::NoProbe,
    variant::{Array, Components, Dict, FloatWidth, Variant},
};
use golf_sim_conditions::{
    grounds::{CourseGround, PracticeGround},
    lies::{AddressLie, BallLie, DivotLie, Sit},
    world::World,
};
use golf_sim_course::course::Course;
use golf_sim_physics::{
    facade as sim,
    plane::{ConstantAir, PlaneGround},
    samplers::{AirSample, AirSampler, GroundSample, GroundSampler},
    values::{Launch, ShotResult, Zone, units},
};
use golf_sim_stroke::{
    address::{Address, Assists},
    delivery::{DeliveryExecution, DeliveryInput},
    results::{DropResult, Preview, StrokeResult},
    stroke::{self, StrokeInput},
};
use golf_sim_swing::spec::{StrikeMode, SwingSpec};
use golf_sim_testkit::tolerance;
use golf_sim_validate::{Conditions, Corpus, Kind, Model, Quantities, Quantity, Status, Target};

const ABSENT: Vec3 = Vec3::new(Real::INFINITY, Real::INFINITY, Real::INFINITY);

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

macro_rules! record {
    ($($key:ident: $value:expr),+ $(,)?) => {
        object([$( (stringify!($key), ($value).into()) ),+])
    };
}

fn field<'a>(input: &'a Dict, name: &str) -> &'a Variant {
    input.get(name).unwrap_or_else(|| panic!("missing {name}"))
}

fn dict(value: &Variant) -> &Dict {
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

fn number(input: &Dict, name: &str) -> Real {
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

fn vector3(value: &Variant) -> Vec3 {
    let Variant::Vector3(vector) = value else {
        panic!("expected a Vector3");
    };
    let [x, y, z] = vector.values;
    Vec3::new(x, y, z)
}

fn vector(point: Vec3) -> Variant {
    Variant::Vector3(Components {
        values: [point.x, point.y, point.z],
        width: FloatWidth::Double,
    })
}

fn array(values: impl IntoIterator<Item = Variant>) -> Variant {
    Variant::Array(Array::new(values.into_iter().collect()))
}

fn ground_sample(input: &Dict) -> GroundSample {
    GroundSample {
        surface: Surface::ALL[code(field(input, "surface"))],
        height_m: number(input, "height_m"),
        normal: vector3(field(input, "normal")),
        firmness: number(input, "firmness"),
        moisture: number(input, "moisture"),
        green_speed_m: number(input, "green_speed_m"),
        grass_height_m: number(input, "grass_height_m"),
        grass_density: number(input, "grass_density"),
    }
}

fn plane(input: &Dict) -> PlaneGround {
    assert_eq!(field(input, "kind").as_str(), Some("plane"));
    let mut ground = PlaneGround::new(
        ground_sample(dict(field(input, "base"))),
        vector3(field(input, "origin")),
    );
    for patch in items(field(input, "patches")).iter().map(dict) {
        ground.add_patch(
            vector2(field(patch, "min")),
            vector2(field(patch, "max")),
            ground_sample(dict(field(patch, "sample"))),
        );
    }
    for trunk in items(field(input, "trunks")).iter().map(dict) {
        ground.add_trunk(
            vector2(field(trunk, "c")),
            number(trunk, "r"),
            number(trunk, "h"),
        );
    }
    for canopy in items(field(input, "canopies")).iter().map(dict) {
        ground.add_canopy(
            vector3(field(canopy, "c")),
            number(canopy, "r"),
            number(canopy, "d"),
        );
    }
    if let Some(zones) = field(input, "zones").as_dict() {
        let kinds = [Zone::InPlay, Zone::PenaltyArea, Zone::OutOfBounds];
        ground.set_outside_zone(kinds[code(field(zones, "outside"))]);
        let Variant::PackedInt32Array(codes) = field(zones, "kinds") else {
            panic!("expected zone kinds");
        };
        let polygons = items(field(zones, "polygons"));
        assert_eq!(codes.len(), polygons.len());
        for (&kind, polygon) in codes.iter().zip(polygons) {
            let Variant::PackedVector2Array(polygon) = polygon else {
                panic!("expected a zone polygon");
            };
            let &[[x0, z0], lower_right, [x1, z1], upper_left] = polygon.values.as_slice() else {
                panic!("expected a rectangular zone");
            };
            assert_eq!(lower_right, [x1, z0]);
            assert_eq!(upper_left, [x0, z1]);
            ground.add_zone(
                Vec2::new(x0, z0),
                Vec2::new(x1, z1),
                kinds[usize::try_from(kind).unwrap()],
            );
        }
    }
    ground
}

fn air(input: &Dict) -> ConstantAir {
    assert_eq!(field(input, "kind").as_str(), Some("constant_air"));
    let sample = dict(field(input, "air"));
    let mut air = ConstantAir::new(AirSample {
        temperature_c: number(sample, "temperature_c"),
        pressure_pa: number(sample, "pressure_pa"),
        relative_humidity: number(sample, "relative_humidity"),
        mean_wind_mps: vector3(field(sample, "mean_wind_mps")),
        gust_mps: vector3(field(sample, "gust_mps")),
        wind_reference_height_m: number(sample, "wind_reference_height_m"),
        roughness_length_m: number(sample, "roughness_length_m"),
    });
    for gust in items(field(input, "gusts")).iter().map(dict) {
        air.add_gust(
            number(gust, "start"),
            number(gust, "end"),
            vector3(field(gust, "gust")),
        );
    }
    air
}

fn world<'a>(input: &Dict, course: &'a Course) -> World<'a> {
    let ground = dict(field(input, "ground"));
    let air = air(dict(field(input, "air")));
    let cup = vector3(field(input, "cup"));
    let cup = cup.is_finite().then_some(cup);
    let clock = number(input, "clock_s");
    match field(ground, "kind").as_str().unwrap() {
        "course_ground" => {
            let identity = dict(field(ground, "course"));
            assert_eq!(
                field(identity, "id").as_str(),
                Some(course.data().id.as_str())
            );
            assert_eq!(
                field(identity, "revision").as_str(),
                Some(course.revision())
            );
            let ground = CourseGround::new(
                course,
                number(ground, "firmness"),
                number(ground, "moisture"),
                number(ground, "green_speed_m"),
            );
            World::on_course(ground, air, cup, clock)
        }
        "practice_ground" => {
            let mut practice = PracticeGround::new(plane(dict(field(ground, "plane"))));
            let gradient = vector2(field(ground, "pad_gradient"));
            practice.tilt_pad(
                vector3(field(ground, "pad_centre")),
                Vec3::new(gradient.x, 0.0, gradient.y),
                gradient.length(),
                number(ground, "flat_radius_m"),
                number(ground, "blend_radius_m"),
            );
            World::practice(
                practice,
                air,
                ball_lie(dict(field(input, "lie"))),
                cup,
                clock,
            )
        }
        "plane" => World::practice(
            plane(ground),
            air,
            ball_lie(dict(field(input, "lie"))),
            cup,
            clock,
        ),
        kind => panic!("unknown ground {kind}"),
    }
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

fn launch_snapshot(launch: Launch) -> Variant {
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

fn stroke_input(input: &Dict) -> StrokeInput {
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

fn execution(input: &Dict) -> DeliveryExecution {
    DeliveryExecution {
        power: number(input, "power"),
        timing_fault: number(input, "timing_fault"),
        timed_out: boolean(field(input, "timed_out")),
        strike_m: boolean(field(input, "has_strike")).then(|| vector2(field(input, "strike_m"))),
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

fn replay(case: &Dict, world: &World<'_>) -> Variant {
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
                "stroke.play" => {
                    stroke_snapshot(&stroke::play(&input, execution(), world, &mut NoProbe))
                }
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

fn fields_mut(value: &mut Variant) -> &mut Dict {
    let Variant::Dict(dict) = value else {
        panic!("expected a dictionary");
    };
    dict
}

fn magnitude(point: Vec3) -> Real {
    point.x.abs().max(point.y.abs()).max(point.z.abs())
}

fn horizontal(point: Vec3) -> Vec2 {
    Vec2::new(point.x, point.z)
}

fn exceeds_default(a: Real, b: Real, scale: Real) -> bool {
    let bound = 1e-6_f64
        .max(1e-5 * a.abs().max(b.abs()))
        .max(8.0 * SINGLE_STEP * scale.max(1.0));
    let error = (a - b).abs();
    error.is_finite() && error > bound
}

struct DerivedChecks<'a> {
    id: &'a str,
}

impl DerivedChecks<'_> {
    fn number(
        &self,
        expected: &Dict,
        actual: &mut Dict,
        key: &str,
        reason: &str,
        allowed: impl FnOnce(Real, Real) -> Real,
    ) {
        let a = number(expected, key);
        let b = number(actual, key);
        let error = (a - b).abs();
        if exceeds_default(a, b, a.abs().max(b.abs())) {
            let bound = allowed(a, b);
            assert!(
                error <= bound,
                "{} {key}: {error} exceeds {bound} ({reason})",
                self.id
            );
            println!("{} {key}: {error:.6e} ≤ {bound:.6e} ({reason})", self.id);
            *actual.get_mut(key).unwrap() = field(expected, key).clone();
        }
    }

    fn launch(&self, expected: &Dict, actual: &mut Dict, standalone: bool) {
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
        &self,
        expected: &Dict,
        actual: &mut Dict,
        key: &str,
        source: [Vec3; 2],
    ) -> bool {
        let a = vector3(field(expected, key));
        let b = vector3(field(actual, key));
        assert_eq!([a, b], source, "{} {key}", self.id);
        let scale = magnitude(a).max(magnitude(b));
        let needs_check = [(a.x, b.x), (a.y, b.y), (a.z, b.z)]
            .into_iter()
            .any(|(a, b)| exceeds_default(a, b, scale));
        if needs_check {
            *actual.get_mut(key).unwrap() = field(expected, key).clone();
        }
        needs_check
    }

    fn outcome(
        &self,
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
                let expected_contact = expected_touches.next().unwrap();
                let actual_contact = actual_touches.next().unwrap();
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
) -> Real {
    let ground = world.ground();
    let air = world.air_mean(input.ball_position());
    let launch = stroke::nominal_shot(input, world).launch.unwrap();
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
    assert_eq!(crossing(height, &rounding), carry);
    // Each flight step stores a new height in a vector. The crossing distance depends
    // on the descent slope, so measure its sensitivity by moving the crossing plane.
    let dy = rounding.height_error.get() + SINGLE_STEP * height.abs().max(1.0);
    (crossing(height - dy, &air) - carry)
        .abs()
        .max((crossing(height + dy, &air) - carry).abs())
}

// Independently check the computation-specific bounds before projecting those values
// to the fixture. All other values, including the operands, keep their table bounds.
fn check_derived_fields(case: &Dict, world: &World<'_>, actual: &mut Variant) {
    let checks = DerivedChecks {
        id: field(case, "id").as_str().unwrap(),
    };
    let expected = dict(field(case, "output"));
    let actual = fields_mut(actual);
    match field(case, "call").as_str().unwrap() {
        "stroke.launch" => checks.launch(expected, actual, true),
        "stroke.play" => {
            let expected_shot = dict(field(expected, "playback"));
            let mut outcome = std::mem::replace(actual.get_mut("outcome").unwrap(), Variant::Nil);
            checks.outcome(
                dict(field(expected, "outcome")),
                fields_mut(&mut outcome),
                expected_shot,
                dict(field(actual, "playback")),
            );
            *actual.get_mut("outcome").unwrap() = outcome;
            let playback = fields_mut(actual.get_mut("playback").unwrap());
            checks.launch(
                dict(field(expected_shot, "launch")),
                fields_mut(playback.get_mut("launch").unwrap()),
                false,
            );
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
                checks.number(
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
}

#[test]
fn every_recorded_stroke_call_matches() {
    let root = testdata();
    let course = Course::open(&root.join("courses/test-course")).unwrap();
    let path = root.join("stroke/facade/fixture.bin");
    let mut counts = std::collections::BTreeMap::new();
    let report = tolerance::check(&path, "stroke", |case| {
        let call = field(case, "call").as_str().unwrap();
        *counts.entry(call.to_owned()).or_insert(0) += 1;
        let args = dict(field(case, "input"));
        let world = world(dict(field(args, "world")), &course);
        let mut actual = replay(case, &world);
        check_derived_fields(case, &world, &mut actual);
        Ok::<_, Infallible>(actual)
    })
    .unwrap();
    assert_eq!(report.checked, 204);
    assert!(report.skipped.is_empty());
    assert!(report.discrete.is_empty(), "{report}");
    print!("{report}");
    for (call, count) in counts {
        println!("{call}: {count} passed");
    }
    println!("{} discrete differences", report.discrete.len());
}

struct StockModel;

impl Model for StockModel {
    fn predict(&self, target: &Target, _: &Conditions) -> Option<Quantities> {
        if target.kind != Kind::StockCarry {
            return None;
        }
        let club = ClubId::from_name(target.inputs["club"].as_str().unwrap()).unwrap();
        let air = ConstantAir::new(AirSample {
            temperature_c: target.inputs["air_temp_c"].as_number().unwrap(),
            pressure_pa: sim::pressure_at_altitude(
                target.inputs["altitude_m"].as_number().unwrap(),
            ),
            ..sim::standard_air()
        });
        let aim = Vec3::new(0.0, 0.0, -1.0);
        let world = World::practice(
            PracticeGround::new(PlaneGround::default()),
            air,
            BallLie::on(Surface::Fairway, Sit::Normal, Vec3::ZERO),
            None,
            0.0,
        );
        let address = stroke::stock(club);
        let ball = stroke::place(Vec3::ZERO, &world, Seed(0), &mut NoProbe);
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
        let shot = stroke::nominal_shot(&input, &world);
        let launch = shot.launch.unwrap().readout;
        let predictions = [
            ("carry_yd", units::to_yards(shot.carry_m())),
            ("ball_mph", units::to_mph(launch.ball_speed_mps)),
            ("launch_deg", launch.launch_angle_rad.to_degrees()),
            ("spin_rpm", units::to_rpm(launch.spin_rate_rad_s)),
        ];
        Some(Quantities::from(predictions.map(|(key, value)| {
            (key.to_owned(), Quantity::Number(value))
        })))
    }
}

#[test]
fn measured_stock_carry() {
    let corpus = Corpus::load(testdata().join("validation")).unwrap();
    let mut report = corpus.run(&StockModel);
    report
        .targets
        .retain(|target| target.kind == Kind::StockCarry);
    report.skipped_datasets.clear();
    print!("{}", report.scorecard());
    assert_eq!(report.targets.len(), 1);
    for target in &report.targets {
        assert_eq!(target.quantities.len(), 4);
        for (name, quantity) in &target.quantities {
            assert_eq!(
                quantity.status,
                Status::Passed,
                "{} {name}: {quantity:#?}",
                target.id
            );
        }
    }
}
