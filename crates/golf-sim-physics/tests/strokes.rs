use golf_sim_base::{
    Real, Surface, Vec3, fm,
    trace::{NoProbe, Phase, Trace},
};
use golf_sim_physics::{
    integrator::{self, Options},
    plane::{ConstantAir, PlaneGround},
    samplers::{AirSample, GroundSample, GroundSampler},
    tunables as t,
    values::{Cup, EndReason, EventKind, Launch, LaunchReadout, ShotResult, units},
};
use golf_sim_validate::{
    Conditions, Corpus, EarlierVia, Kind, Model, Quantities, Quantity, Status, Submodel, Target,
};

const FORWARD: Vec3 = Vec3::new(0.0, 0.0, -1.0);
const UP: Vec3 = Vec3::new(0.0, 1.0, 0.0);
const STIMP_10_M: Real = 3.048;

fn calm() -> ConstantAir {
    ConstantAir::new(AirSample {
        temperature_c: 25.0,
        ..AirSample::default()
    })
}

fn green(stimp_m: Real, grade: Real) -> PlaneGround {
    let sample = GroundSample {
        surface: Surface::Green,
        green_speed_m: stimp_m,
        ..GroundSample::default()
    };
    if grade == 0.0 {
        PlaneGround::new(sample, Vec3::ZERO)
    } else {
        PlaneGround::sloped(sample, FORWARD, grade)
    }
}

fn launch(at: Vec3, aim: Vec3, speed: Real, angle_deg: Real, spin: Real) -> Launch {
    Launch::from_readout(
        &t::BALL,
        at,
        aim,
        LaunchReadout {
            ball_speed_mps: speed,
            launch_angle_rad: angle_deg.to_radians(),
            spin_rate_rad_s: spin,
            ..LaunchReadout::default()
        },
    )
}

fn simulate(
    launch: Launch,
    air: &ConstantAir,
    ground: &PlaneGround,
    cup: Option<Cup>,
) -> ShotResult {
    integrator::run(
        &launch,
        &t::BALL,
        air,
        ground,
        cup,
        Options::default(),
        &mut NoProbe,
    )
}

fn roll(speed: Real, dir: Vec3, ground: &PlaneGround, cup: Option<Cup>) -> ShotResult {
    let mut launch = launch(Vec3::ZERO, dir, speed, 0.0, -speed / t::BALL.radius_m);
    launch.position.y = ground.sample_ground(0.0, 0.0).height_m + t::BALL.radius_m;
    simulate(launch, &calm(), ground, cup)
}

fn in_scope(target: &Target) -> bool {
    matches!(
        target.kind,
        Kind::TurfImpact | Kind::GreenSpeed | Kind::CupCapture | Kind::RunOut
    ) && target
        .earlier
        .as_ref()
        .is_some_and(|earlier| earlier.via == EarlierVia::Production)
}

struct StrokeModel;

impl Model for StrokeModel {
    fn predict(&self, target: &Target, _: &Conditions) -> Option<Quantities> {
        if !in_scope(target) {
            return None;
        }
        let input = |name: &str| {
            target.inputs[name]
                .as_number()
                .unwrap_or_else(|| panic!("{}: {name} must be a number", target.id))
        };
        let number = |name: &str, value| (name.to_owned(), Quantity::Number(value));
        let predictions = match target.kind {
            Kind::TurfImpact => {
                let launch = launch(
                    UP * 0.01,
                    FORWARD,
                    input("speed_mps"),
                    -input("angle_deg"),
                    input("spin_rad_s"),
                );
                let stroke = simulate(launch, &calm(), &green(STIMP_10_M, 0.0), None);
                let contact = stroke
                    .first_event(EventKind::Bounce)
                    .or_else(|| stroke.first_event(EventKind::RollStart))
                    .expect("turf launch had no bounce or roll");
                let horizontal = Vec3::new(contact.velocity.x, 0.0, contact.velocity.z).length();
                Quantities::from([
                    number("out_speed_mps", contact.velocity.length()),
                    number(
                        "out_angle_deg",
                        fm::atan2(contact.velocity.y, horizontal).to_degrees(),
                    ),
                    number("out_spin_rad_s", contact.spin.x),
                ])
            }
            Kind::GreenSpeed => {
                let stimp = units::feet(input("stimp_ft"));
                let speed = input("exit_mps");
                let (name, distance) = if target.inputs.contains_key("grade") {
                    let slope = green(stimp, input("grade"));
                    let up = roll(speed, FORWARD, &slope, None).total_m();
                    let down = roll(speed, -FORWARD, &slope, None).total_m();
                    ("brede_ft", 2.0 * up * down / (up + down))
                } else {
                    (
                        "roll_ft",
                        roll(speed, FORWARD, &green(stimp, 0.0), None).total_m(),
                    )
                };
                Quantities::from([number(name, units::to_feet(distance))])
            }
            Kind::CupCapture => {
                let radius = t::CUP_RADIUS_M;
                let offset = input("offset_radii") * radius;
                let cup = Cup {
                    position: Vec3::new(offset, 0.0, -2.0),
                    flagstick_in: false,
                };
                let rim = 2.0 - fm::sqrt(radius * radius - offset * offset);
                let exit = input("exit_mps");
                let entry = input("entry_mps");
                let decel = exit * exit / (2.0 * STIMP_10_M);
                let speed = fm::sqrt(entry * entry + 2.0 * decel * rim);
                let stroke = roll(speed, FORWARD, &green(STIMP_10_M, 0.0), Some(cup));
                Quantities::from([
                    (
                        "holed".into(),
                        Quantity::Bool(stroke.end_reason == EndReason::Holed),
                    ),
                    number(
                        "entry_mps",
                        stroke
                            .first_event(EventKind::CupEntry)
                            .map_or(0.0, |event| event.velocity.length()),
                    ),
                ])
            }
            Kind::RunOut => {
                let air = if target.inputs.contains_key("wind_mph") {
                    ConstantAir::new(AirSample {
                        mean_wind_mps: Vec3::new(0.0, 0.0, units::mph(input("wind_mph"))),
                        roughness_length_m: 1e-6,
                        wind_reference_height_m: 15.0,
                        ..calm().air
                    })
                } else {
                    calm()
                };
                let launch = launch(
                    Vec3::ZERO,
                    FORWARD,
                    units::mph(input("ball_mph")),
                    input("launch_deg"),
                    input("spin_rpm") * units::RPM,
                );
                let stroke = simulate(launch, &air, &PlaneGround::default(), None);
                Quantities::from([number("run_yd", units::to_yards(stroke.run_m()))])
            }
            _ => unreachable!(),
        };
        Some(predictions)
    }
}

#[test]
fn measured_strokes() {
    let corpus = Corpus::load_default(None).unwrap();
    let mut report = corpus.run(&StrokeModel);
    let relevant = |submodel| {
        matches!(
            submodel,
            Submodel::TurfBounce | Submodel::RunoutGreens | Submodel::Cup
        )
    };
    let expected = corpus
        .files()
        .iter()
        .flat_map(|file| &file.targets)
        .filter(|target| in_scope(target))
        .count();
    assert!(expected > 0);
    let mut scorecard = report.scorecard();
    scorecard.rows.retain(|row| relevant(row.key.submodel));
    scorecard.submodels.retain(|row| relevant(row.submodel));
    println!("{scorecard}");
    report
        .targets
        .retain(|target| target.status != Status::NotRun);
    assert_eq!(report.targets.len(), expected);
    let mut failures = Vec::new();
    for target in &report.targets {
        for (name, result) in &target.quantities {
            if (result.source_tolerance.is_some() || result.earlier_prediction.is_some())
                && result.status != Status::Passed
            {
                let earlier_residual = result
                    .earlier_prediction
                    .as_ref()
                    .and_then(Quantity::as_number)
                    .zip(result.measured.as_number())
                    .map(|(earlier, measured)| earlier - measured);
                failures.push(format!("{} {name}: predicted {:?}, measured {}, residual {:?}, earlier {:?}, earlier residual {:?}",
                    target.id, result.predicted, result.measured, result.residual, result.earlier_prediction, earlier_residual));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    println!("{expected} measured launches passed");
}

#[test]
fn trace_covers_launch_flight_bounce_and_roll() {
    let launch = launch(Vec3::ZERO, FORWARD, 55.0, 14.0, 300.0);
    let mut trace = Trace::default();
    let result = integrator::run(
        &launch,
        &t::BALL,
        &calm(),
        &PlaneGround::default(),
        None,
        Options::default(),
        &mut trace,
    );
    assert_eq!(result.end_reason, EndReason::Rest);
    for phase in [
        Phase::Launch,
        Phase::Flight,
        Phase::Bounce,
        Phase::Skid,
        Phase::Roll,
    ] {
        assert!(
            trace
                .phases()
                .iter()
                .any(|entry| entry.phase == phase && !entry.values.is_empty()),
            "missing {phase:?}"
        );
    }
}

#[test]
fn timeout_plays_back_the_airborne_ball_and_places_rest_on_the_ground() {
    let launch = Launch {
        position: UP * 60_000.0,
        ..Launch::default()
    };
    let ground = PlaneGround::default();
    let vacuum = ConstantAir::new(AirSample {
        pressure_pa: 0.0,
        ..AirSample::default()
    });
    let stroke = integrator::run(
        &launch,
        &t::BALL,
        &vacuum,
        &ground,
        None,
        Options::default(),
        &mut NoProbe,
    );
    assert_eq!(stroke.end_reason, EndReason::Timeout);
    assert!(stroke.touchdowns.is_empty());
    assert!(stroke.positions.last().unwrap().y > 30_000.0);
    assert_eq!(stroke.rest_position, UP * t::BALL.radius_m);
}

#[test]
fn zone_crossings_stamp_playback_without_ending_play() {
    use golf_sim_base::Vec2;
    use golf_sim_physics::values::Zone;

    let mut ground = PlaneGround::default();
    ground.add_zone(
        Vec2::new(-1.0, -2.0),
        Vec2::new(1.0, -1.0),
        Zone::PenaltyArea,
    );
    let launch = launch(Vec3::ZERO, FORWARD, 3.0, 0.0, -3.0 / t::BALL.radius_m);
    let stroke = integrator::run(
        &launch,
        &t::BALL,
        &calm(),
        &ground,
        None,
        Options::default(),
        &mut NoProbe,
    );
    assert_eq!(stroke.end_reason, EndReason::Rest);
    let crossings: Vec<_> = stroke
        .events
        .iter()
        .filter(|event| event.kind == EventKind::ZoneEntered)
        .collect();
    assert_eq!(crossings.len(), 2);
    for (event, zone, z) in [
        (crossings[0], Zone::PenaltyArea, -1.0),
        (crossings[1], Zone::InPlay, -2.0),
    ] {
        assert_eq!(event.zone, zone);
        assert!((event.position.z - z).abs() < 1e-12);
        assert_eq!(stroke.position_at(event.time_s), event.position);
    }
    assert!(stroke.times.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn carry_to_level_agrees_with_first_touchdown_on_flat_ground() {
    let launch = launch(Vec3::ZERO, FORWARD, 55.0, 14.0, 300.0);
    let ground = PlaneGround::default();
    let air = calm();
    let stroke = simulate(launch, &air, &ground, None);
    let carry = integrator::carry_to_height(&launch, &t::BALL, 0.0, &air, &ground, 0.0);
    assert!((carry - stroke.carry_m()).abs() < 1e-12);
}

#[test]
fn a_ground_rebound_uses_the_normal_at_the_obstacle() {
    use golf_sim_physics::values::{ObstacleHit, ObstacleKind};

    struct WallAtRamp;

    impl GroundSampler for WallAtRamp {
        fn sample_ground(&self, _: Real, z: Real) -> GroundSample {
            if z > -0.01 {
                GroundSample::default()
            } else {
                GroundSample {
                    // Keep ball support continuous so rolling off the ramp cannot cause the hop.
                    height_m: (-0.01 - z) * 0.75 + t::BALL.radius_m * (1.0 - 1.0 / 0.8),
                    normal: Vec3::new(0.0, 0.8, 0.6),
                    ..GroundSample::default()
                }
            }
        }

        fn obstacle_hit(&self, from: Vec3, to: Vec3, _: Real) -> Option<ObstacleHit> {
            let wall_z = -0.012;
            (from.z > wall_z && to.z <= wall_z).then(|| {
                let fraction = (wall_z - from.z) / (to.z - from.z);
                ObstacleHit {
                    kind: ObstacleKind::Wall,
                    point: from.lerp(to, fraction),
                    normal: -FORWARD,
                    fraction,
                }
            })
        }
    }

    let mut trace = Trace::default();
    let launch = launch(Vec3::ZERO, FORWARD, 4.0, 0.0, 0.0);
    let stroke = integrator::run(
        &launch,
        &t::BALL,
        &calm(),
        &WallAtRamp,
        None,
        Options::default(),
        &mut trace,
    );
    assert_eq!(stroke.events[1].kind, EventKind::RollStart);
    assert!(stroke.first_event(EventKind::Obstacle).is_some());
    assert!(
        trace
            .phases()
            .iter()
            .any(|phase| phase.phase == Phase::Flight)
    );
}
