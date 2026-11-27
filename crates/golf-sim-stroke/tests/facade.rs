use std::path::{Path, PathBuf};

use golf_sim_base::{ClubId, Real, Seed, Surface, Vec3, trace::NoProbe, variant::Variant};
use golf_sim_conditions::{
    grounds::PracticeGround,
    lies::{BallLie, Sit},
    world::World,
};
use golf_sim_physics::{
    facade as sim,
    plane::{ConstantAir, PlaneGround},
    samplers::{AirSample, GroundSample},
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
    let fixture = corpus::FixtureRun::read(&path, None, &[]).unwrap();
    let case_count = fixture.cases().len();
    let report = fixture
        .check_observing(|case| {
            let call = case.get("call").and_then(Variant::as_str).unwrap();
            *counts.entry(call.to_owned()).or_insert(0) += 1;
        })
        .unwrap();
    assert_eq!(report.checked + report.skipped.len(), case_count);
    assert_eq!(report.skipped.len(), 42);
    assert!(report.discrete.is_empty(), "{report}");
    print!("{report}");
    for (call, count) in counts {
        println!("{call}: {count} passed");
    }
    println!("{} discrete differences", report.discrete.len());
}

struct StockModel;

fn stock_carry(club: ClubId, lie: BallLie) -> Real {
    let sample = GroundSample {
        surface: lie.surface,
        ..GroundSample::default()
    };
    let world = World::practice(
        PlaneGround::new(sample, Vec3::ZERO),
        ConstantAir::default(),
        lie,
        None,
        0.0,
    );
    let aim = Vec3::new(0.0, 0.0, -1.0);
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
    stroke::nominal_shot(&input, &world).carry_m()
}

fn fairway_carry(club: ClubId) -> Real {
    stock_carry(club, BallLie::on(Surface::Fairway, Sit::Normal, Vec3::ZERO))
}

#[test]
fn a_wedge_or_short_iron_advances_out_of_native_fescue() {
    use ClubId::{Hybrid, Iron5, Iron7, Iron9, LobWedge, PitchingWedge, SandWedge};

    let clubs = [
        LobWedge,
        SandWedge,
        PitchingWedge,
        Iron9,
        Iron7,
        Iron5,
        Hybrid,
    ];
    let fairway = clubs.map(fairway_carry);
    for sit in [Sit::Up, Sit::Normal, Sit::Down] {
        let mut lie = BallLie::on(Surface::DeepRough, sit, Vec3::ZERO);
        lie.set_grass(0.42, None);
        let shares = std::array::from_fn::<_, 7, _>(|i| stock_carry(clubs[i], lie) / fairway[i]);
        println!("{sit:?} fescue carry shares: {shares:.3?}");
        for (club, share) in clubs.into_iter().zip(shares).take(3) {
            assert!((0.4..=0.8).contains(&share), "{club:?} {sit:?}: {share:.3}");
        }
        let short_iron = shares[3];
        assert!(
            (0.25..=0.6).contains(&short_iron),
            "9-iron {sit:?}: {short_iron:.3}"
        );
        for (club, share) in clubs.into_iter().zip(shares).skip(4) {
            assert!(
                share < short_iron,
                "{club:?} {sit:?}: {share:.3} >= {short_iron:.3}"
            );
        }
    }
}

#[test]
fn a_wedge_only_hacks_a_ball_out_of_marsh_sedge() {
    use ClubId::{Hybrid, Iron5, Iron9, SandWedge};

    let mut lie = BallLie::on(Surface::DeepRough, Sit::Normal, Vec3::ZERO);
    lie.set_grass(0.5, Some(0.55));
    let [sand_wedge, iron9, iron5, hybrid] =
        [SandWedge, Iron9, Iron5, Hybrid].map(|club| stock_carry(club, lie) / fairway_carry(club));
    println!(
        "Marsh carry shares (SW, 9i, 5i, hybrid): {sand_wedge:.3}, {iron9:.3}, {iron5:.3}, {hybrid:.3}"
    );
    assert!((0.15..=0.5).contains(&sand_wedge));
    lie.set_grass(0.42, None);
    assert!(sand_wedge + 0.1 < stock_carry(SandWedge, lie) / fairway_carry(SandWedge));
    assert!(iron9 < 0.1);
    assert!(iron5 < 0.05);
    assert!(hybrid < 0.05);
}

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
    let corpus = Corpus::load_default(None).unwrap();
    let mut report = corpus.run(&StockModel);
    report
        .targets
        .retain(|target| target.kind == Kind::StockCarry);
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
