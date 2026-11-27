use std::path::{Path, PathBuf};

use golf_sim_base::{ClubId, Seed, Surface, Vec3, trace::NoProbe, variant::Variant};
use golf_sim_conditions::{
    grounds::PracticeGround,
    lies::{BallLie, Sit},
    world::World,
};
use golf_sim_physics::{
    facade as sim,
    plane::{ConstantAir, PlaneGround},
    samplers::AirSample,
    values::units,
};
use golf_sim_stroke::{
    delivery::DeliveryInput,
    stroke::{self, StrokeInput},
};
use golf_sim_testkit::corpus;
use golf_sim_validate::{Conditions, Corpus, Kind, Model, Quantities, Quantity, Status, Target};

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

#[test]
fn every_recorded_stroke_call_matches() {
    let path = testdata().join("stroke/facade/fixture.bin");
    let mut counts = std::collections::BTreeMap::new();
    let report = corpus::FixtureRun::read(&path, None, &[])
        .unwrap()
        .check_observing(|case| {
            let call = case.get("call").and_then(Variant::as_str).unwrap();
            *counts.entry(call.to_owned()).or_insert(0) += 1;
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
