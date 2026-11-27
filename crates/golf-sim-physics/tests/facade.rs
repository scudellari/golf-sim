use std::path::{Path, PathBuf};

use golf_sim_base::{
    ClubId, Real, Surface, Vec2, Vec3, fm,
    trace::{NoProbe, Phase, Trace},
    variant::{self, Variant},
};
use golf_sim_physics::{
    facade as sim,
    plane::{ConstantAir, PlaneGround},
    samplers::{AirSample, GroundSample},
    values::{ClubCategory, Delivery, EventKind, Lie, ShotRequest, units},
};
use golf_sim_testkit::corpus;
use golf_sim_validate::{Conditions, Corpus, Kind, Model, Quantities, Quantity, Status, Target};

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

#[test]
fn matches_facade_fixture() {
    let path = testdata().join("physics/facade/fixture.bin");
    let mut golden = 0;
    let report = corpus::FixtureRun::read(&path, None, &[])
        .unwrap()
        .check_observing(|case| {
            let id = case.get("id").and_then(Variant::as_str).unwrap();
            if id.contains("/test_golden_stroke_is_unchanged/")
                && case.get("call").and_then(Variant::as_str) == Some("golf_sim.simulate_shot")
            {
                golden += 1;
            }
        })
        .unwrap();
    assert_eq!(golden, 1);
    assert!(report.checked > 0);
    let generated: Vec<_> = report
        .skipped
        .iter()
        .filter(|entry| entry.change == "requires the day generator")
        .map(|entry| entry.id.as_str())
        .collect();
    for id in [
        "play/test-course/2/000054",
        "play/test-course/2/011592",
        "play/test-course/2/017232",
    ] {
        assert!(generated.contains(&id), "{id}");
    }
    let fixture = golf_sim_base::corpus::read(&path).unwrap();
    let Some(Variant::Array(cases)) = fixture.as_dict().unwrap().get("cases") else {
        panic!("expected fixture cases");
    };
    assert_eq!(cases.items.len(), report.checked + report.skipped.len());
    for case in &cases.items {
        let case = case.as_dict().unwrap();
        let id = case.get("id").and_then(Variant::as_str).unwrap();
        if generated.contains(&id) {
            let input = case.get("input").and_then(Variant::as_dict).unwrap();
            for (name, kind) in [
                ("conditions", "generated_air"),
                ("ground", "generated_ground"),
            ] {
                assert_eq!(
                    input
                        .get(name)
                        .and_then(Variant::as_dict)
                        .and_then(|sampler| sampler.get("kind"))
                        .and_then(Variant::as_str),
                    Some(kind),
                    "{id}",
                );
            }
        }
    }
    print!("{report}");
}

fn target_number(target: &Target, name: &str) -> Option<Real> {
    target.inputs.get(name).map(|value| {
        value
            .as_number()
            .unwrap_or_else(|| panic!("{} expected a number for {name}", target.id))
    })
}

fn validation_delivery(target: &Target, loft_deg: Real) -> Delivery {
    let input = |key| target_number(target, key);
    Delivery {
        club_speed_mps: units::mph(input("club_mph").unwrap()),
        attack_rad: input("attack_deg").unwrap().to_radians(),
        dynamic_loft_rad: loft_deg.to_radians(),
        path_rad: input("path_deg").unwrap_or_default().to_radians(),
        face_rad: input("face_deg").unwrap_or_default().to_radians(),
        ..Delivery::default()
    }
}

fn delivery_request(club_id: ClubId, mut delivery: Delivery) -> ShotRequest {
    let club = sim::standard_club(club_id);
    let ball = sim::standard_ball();
    let lie = if club.category == ClubCategory::Driver {
        let lie = Lie::tee(0.035);
        delivery.sole_height_m = sim::sole_height_for_strike(&club, &ball, &delivery, &lie, 0.0);
        lie
    } else {
        Lie {
            ball_height_m: 0.005,
            ..Lie::default()
        }
    };
    ShotRequest::new(club, ball, delivery, lie)
}

fn delivery_loft(target: &Target, club: ClubId, air: &ConstantAir, ground: &PlaneGround) -> Real {
    if let Some(loft) = target_number(target, "dynamic_loft_deg") {
        return loft;
    }
    let launch_deg = target_number(target, "launch_deg").unwrap();
    let (mut low, mut high) = (0.0, 50.0);
    for _ in 0..30 {
        let mid = (low + high) * 0.5;
        let mut delivery = validation_delivery(target, mid);
        delivery.path_rad = 0.0;
        delivery.face_rad = 0.0;
        let request = delivery_request(club, delivery);
        let launch = sim::strike(&request, air, ground, &mut NoProbe);
        if launch.readout.launch_angle_rad.to_degrees() < launch_deg {
            low = mid;
        } else {
            high = mid;
        }
    }
    (low + high) * 0.5
}

fn covered(target: &Target, name: &str) -> bool {
    target.kind == Kind::Delivery
        && if target.inputs.get("club").and_then(Quantity::as_str) == Some("putter") {
            matches!(name, "launch_deg" | "speed_ratio" | "true_roll_in")
        } else {
            matches!(name, "carry_yd" | "curve_yd")
        }
}

struct FacadeModel;

impl Model for FacadeModel {
    fn predict(&self, target: &Target, conditions: &Conditions) -> Option<Quantities> {
        if !target.measured.keys().any(|name| covered(target, name)) {
            return None;
        }
        let club = ClubId::from_name(target.inputs["club"].as_str().unwrap()).unwrap();
        let air = ConstantAir::new(AirSample {
            temperature_c: 25.0,
            ..sim::standard_air()
        });
        let number = |key: &str, value| (key.to_owned(), Quantity::Number(value));
        if club == ClubId::Putter {
            let loft = target_number(target, "dynamic_loft_deg").unwrap();
            let delivery = validation_delivery(target, loft);
            let lie = Lie {
                surface: Surface::Green,
                ..Lie::default()
            };
            let request = ShotRequest::new(
                sim::standard_club(club),
                sim::standard_ball(),
                delivery,
                lie,
            );
            let green = PlaneGround::new(
                GroundSample {
                    surface: Surface::Green,
                    green_speed_m: 3.048,
                    ..GroundSample::default()
                },
                Vec3::ZERO,
            );
            let stroke = sim::simulate_shot(&request, &air, &green, true, &mut NoProbe);
            let readout = stroke.launch.unwrap().readout;
            let roll = stroke.first_event(EventKind::PureRoll).unwrap();
            let true_roll_m = Vec2::new(roll.position.x, roll.position.z).length();
            return Some(Quantities::from([
                number("launch_deg", readout.launch_angle_rad.to_degrees()),
                number(
                    "speed_ratio",
                    readout.ball_speed_mps / delivery.club_speed_mps,
                ),
                number("true_roll_in", units::to_feet(true_roll_m) * 12.0),
            ]));
        }

        let ground = PlaneGround::default();
        let loft = delivery_loft(target, club, &air, &ground);
        let request = delivery_request(club, validation_delivery(target, loft));
        let stroke = sim::simulate_shot(&request, &air, &ground, true, &mut NoProbe);
        let carry_m = stroke.carry_m();
        let carry_yd = units::to_yards(carry_m);
        let mut predictions = Quantities::from([number("carry_yd", carry_yd)]);
        if target.measured.contains_key("curve_yd") {
            let direction = stroke.launch.unwrap().readout.launch_direction_rad;
            let curve_yd = units::to_yards(stroke.carry_side_m() - carry_m * fm::tan(direction));
            let curve_yd = conditions.normalize_curve(target, curve_yd, carry_yd);
            predictions.extend([number("curve_yd", curve_yd)]);
        }
        Some(predictions)
    }
}

#[test]
fn measured_facade_targets() {
    let corpus = Corpus::load_default(None).unwrap();
    let targets: std::collections::BTreeMap<_, _> = corpus
        .files()
        .iter()
        .flat_map(|file| &file.targets)
        .map(|target| (target.id.as_str(), target))
        .collect();
    let mut report = corpus.run(&FacadeModel);
    for result in &mut report.targets {
        result
            .quantities
            .retain(|name, _| covered(targets[result.id.as_str()], name));
        result.status = result
            .quantities
            .values()
            .map(|result| result.status)
            .max()
            .unwrap_or(Status::NotRun);
    }
    report
        .targets
        .retain(|result| !result.quantities.is_empty());
    print!("{}", report.scorecard());
    let mut failed = Vec::new();
    for target in &report.targets {
        for (name, result) in &target.quantities {
            if (result.source_tolerance.is_some() || result.earlier_prediction.is_some())
                && result.status != Status::Passed
            {
                failed.push(format!(
                    "{} {name} predicted {:?}, measured {}, residual {:?}, allowed earlier residual {:?}",
                    target.id, result.predicted, result.measured, result.residual, result.allowed_earlier_residual
                ));
            }
        }
    }
    assert!(!report.targets.is_empty());
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    println!("{} measured facade targets passed", report.targets.len());
}

#[test]
fn facade_traces_a_delivery_through_rest_without_changing_the_stroke() {
    let request = delivery_request(
        ClubId::Driver,
        Delivery {
            club_speed_mps: 50.0,
            dynamic_loft_rad: 13.0_f64.to_radians(),
            ..Delivery::default()
        },
    );
    let air = ConstantAir::default();
    let ground = PlaneGround::default();
    let plain = sim::simulate_shot(&request, &air, &ground, true, &mut NoProbe);
    let mut trace = Trace::default();
    let traced = sim::simulate_shot(&request, &air, &ground, true, &mut trace);
    assert_eq!(
        variant::encode(&corpus::physics::shot_snapshot(&plain)).unwrap(),
        variant::encode(&corpus::physics::shot_snapshot(&traced)).unwrap(),
    );
    for phase in [
        Phase::Delivery,
        Phase::Impact,
        Phase::Launch,
        Phase::Flight,
        Phase::Bounce,
        Phase::Roll,
    ] {
        assert!(
            trace.phases().iter().any(|record| record.phase == phase),
            "no {phase:?}"
        );
    }
    let carry = sim::carry_to_height(
        &plain.launch.unwrap(),
        &request.ball,
        request.time_s,
        &air,
        &ground,
        0.0,
    );
    assert!((carry - plain.carry_m()).abs() < 1e-12);
}
