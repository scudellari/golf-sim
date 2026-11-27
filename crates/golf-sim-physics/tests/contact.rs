use std::path::{Path, PathBuf};

use golf_sim_base::{
    Real, Surface, Vec3,
    trace::{NoProbe, Phase, Source, Trace},
};
use golf_sim_physics::{
    bounce, cup,
    roll::{self, Motion},
    samplers::GroundSample,
    tunables as t,
    values::{Cup, units},
};
use golf_sim_testkit::corpus::FixtureRun;
use golf_sim_validate::{
    Conditions, Corpus, EarlierVia, Kind, Model, Quantities, Quantity, Status, Submodel, Target,
};

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

#[test]
fn contact_matches_physics_fixture() {
    let path = testdata().join("physics/contact/fixture.bin");
    let fixture = FixtureRun::read(&path, None, &[]).unwrap();
    let case_count = fixture.cases().len();
    match fixture.check() {
        Ok(report) => {
            assert_eq!(report.checked, case_count);
            assert!(report.skipped.is_empty());
            println!("{report}");
        }
        Err(error) => {
            let drift = FixtureRun::read(&path, None, &[]).unwrap().drift().unwrap();
            println!("{drift}");
            panic!("{error}");
        }
    }
}

struct ContactModel;

impl Model for ContactModel {
    fn predict(&self, target: &Target, conditions: &Conditions) -> Option<Quantities> {
        // Production predictions begin at launch and take their contact data from the stroke.
        (target.submodel == Submodel::TurfBounce
            && target.kind == Kind::TurfImpact
            && target
                .earlier
                .as_ref()
                .is_some_and(|earlier| earlier.via == EarlierVia::FitModel))
        .then(|| predict_bounce(target, conditions))
    }
}

fn input(target: &Target, name: &str) -> Real {
    target.inputs[name].as_number().unwrap()
}

fn predict_bounce(target: &Target, conditions: &Conditions) -> Quantities {
    // The fit predicts both Biber turf types using the same fairway conditions.
    let ground = GroundSample {
        surface: Surface::Fairway,
        firmness: conditions.ground.firmness,
        moisture: conditions.ground.moisture,
        ..GroundSample::default()
    };
    let out = bounce::impact(
        Vec3::new(input(target, "vx_mps"), input(target, "vy_mps"), 0.0),
        Vec3::new(0.0, 0.0, input(target, "spin_rad_s")),
        &ground,
        &t::BALL,
        &mut NoProbe,
    );
    [
        ("out_vx_mps", out.velocity.x),
        ("out_vy_mps", out.velocity.y),
        ("out_spin_rad_s", out.spin.z),
    ]
    .into_iter()
    .map(|(name, value)| (name.into(), Quantity::Number(value)))
    .collect()
}

#[test]
fn contact_measured_targets() {
    let corpus = Corpus::load_default(None).unwrap();
    let report = corpus.run(&ContactModel);
    let mut scores = report.scorecard();
    let in_scope = |model| {
        matches!(
            model,
            Submodel::TurfBounce | Submodel::RunoutGreens | Submodel::Cup
        )
    };
    scores.rows.retain(|row| in_scope(row.key.submodel));
    scores.submodels.retain(|row| in_scope(row.submodel));
    println!("{scores}");

    let mut failures = 0;
    for target in report
        .targets
        .iter()
        .filter(|target| in_scope(target.submodel))
    {
        let covered = target.source == "BIB";
        assert_eq!(target.status != Status::NotRun, covered, "{}", target.id);
        for (quantity, value) in &target.quantities {
            if covered && value.status == Status::Failed {
                let earlier = match (&value.earlier_prediction, &value.measured) {
                    (Some(Quantity::Number(prediction)), Quantity::Number(measurement)) => {
                        Some(prediction - measurement)
                    }
                    _ => None,
                };
                println!(
                    "{} {quantity} residual {:?}, earlier residual {earlier:?}, measured {:?}, predicted {:?}",
                    target.id, value.residual, value.measured, value.predicted
                );
                failures += 1;
            }
        }
    }
    assert_eq!(
        failures, 0,
        "measured contact quantities outside the pass rule"
    );
}

fn vector_bits(vector: Vec3) -> [u64; 3] {
    [vector.x, vector.y, vector.z].map(Real::to_bits)
}

#[test]
fn traced_chip_records_contact_without_changing_bits() {
    let ground = GroundSample {
        surface: Surface::Green,
        green_speed_m: 3.05,
        ..GroundSample::default()
    };
    let velocity = Vec3::new(9.0, -7.0, 0.0);
    let spin = Vec3::new(0.0, 0.0, units::rpm(3500.0));
    let mut trace = Trace::default();
    let mut traced = bounce::impact(velocity, spin, &ground, &t::BALL, &mut trace);
    let mut plain = bounce::impact(velocity, spin, &ground, &t::BALL, &mut NoProbe);
    for _ in 0..2 {
        assert_eq!(vector_bits(traced.velocity), vector_bits(plain.velocity));
        assert_eq!(vector_bits(traced.spin), vector_bits(plain.spin));
        // On a plane a ballistic hop returns with the vertical velocity reversed.
        traced = bounce::impact(
            Vec3::new(traced.velocity.x, -traced.velocity.y, traced.velocity.z),
            traced.spin,
            &ground,
            &t::BALL,
            &mut trace,
        );
        plain = bounce::impact(
            Vec3::new(plain.velocity.x, -plain.velocity.y, plain.velocity.z),
            plain.spin,
            &ground,
            &t::BALL,
            &mut NoProbe,
        );
    }
    assert_eq!(vector_bits(traced.velocity), vector_bits(plain.velocity));
    assert_eq!(vector_bits(traced.spin), vector_bits(plain.spin));

    let mut contact = roll::State {
        velocity: traced.velocity,
        spin: traced.spin,
        motion: Motion::Skid,
    };
    let mut untraced = contact;
    let air = roll::Air {
        density: 1.2,
        viscosity: 1.8e-5,
        half_area_per_mass: 0.5 * t::BALL.area() / t::BALL.mass_kg,
    };
    for _ in 0..2_400 {
        roll::step(
            &mut contact,
            &ground,
            &t::BALL,
            &air,
            t::GROUND_DT,
            &mut trace,
        );
        roll::step(
            &mut untraced,
            &ground,
            &t::BALL,
            &air,
            t::GROUND_DT,
            &mut NoProbe,
        );
        assert_eq!(
            vector_bits(contact.velocity),
            vector_bits(untraced.velocity)
        );
        assert_eq!(vector_bits(contact.spin), vector_bits(untraced.spin));
        assert_eq!(contact.motion, untraced.motion);
        if contact.motion == Motion::Rest {
            break;
        }
    }
    assert_eq!(contact.motion, Motion::Rest);

    let bounces: Vec<_> = trace
        .phases()
        .iter()
        .filter(|entry| entry.phase == Phase::Bounce)
        .collect();
    assert_eq!(bounces.len(), 3);
    for (i, entry) in bounces.iter().enumerate() {
        let get = |name| {
            entry
                .values
                .iter()
                .find(|value| value.name == name)
                .unwrap()
        };
        let cor = get("normal_restitution");
        let spin_in = get("spin_in");
        let spin_out = get("spin_out");
        assert!((0.0..=0.95).contains(&cor.value));
        assert_ne!(spin_in.value, spin_out.value);
        assert!(get("incidence_speed").value > 0.0);
        assert!(get("incidence_angle").value > 0.0);
        assert_eq!(
            get("firmness").source,
            Source::Turf {
                name: "firmness".into()
            }
        );
        assert_eq!(
            get("surface").source,
            Source::Turf {
                name: "GREEN".into()
            }
        );
        assert_eq!(
            get("green_cor_floor").source,
            Source::Tunable {
                name: "BOUNCE_GREEN_COR_FLOOR".into()
            }
        );
        let tangential = get("tangential_restitution");
        assert_eq!(
            tangential.source,
            Source::Tunable {
                name: "BOUNCE_GREEN_TANGENTIAL_COR".into()
            }
        );
        println!(
            "bounce {} normal restitution {:.6}, spin {:.3} → {:.3} rpm (BOUNCE_GREEN_COR_FLOOR={}, BOUNCE_GREEN_TANGENTIAL_COR={})",
            i + 1,
            cor.value,
            spin_in.value,
            spin_out.value,
            get("green_cor_floor").value,
            tangential.value,
        );
    }
    let has = |phase, name| {
        trace.phases().iter().any(|entry| {
            entry.phase == phase && entry.values.iter().any(|value| value.name == name)
        })
    };
    assert!(has(Phase::Skid, "skid_friction"));
    assert!(has(Phase::Roll, "rolling_started"));
    assert!(has(Phase::Roll, "stopped_below_speed_threshold"));
}

#[test]
fn cup_traces_capture_lip_out_and_flagstick() {
    let entry = Vec3::new(-t::CUP_RADIUS_M, t::BALL.radius_m, 0.0);
    let cup = Cup::default();
    let mut trace = Trace::default();
    for (speed, flagstick_in, event) in [
        (1.0, false, "capture"),
        (2.0, false, "lip_out"),
        (1.0, true, "crossing_to_flagstick"),
    ] {
        let cup = Cup {
            flagstick_in,
            ..cup
        };
        let velocity = Vec3::new(speed, 0.0, 0.0);
        let traced = cup::putt_entry(entry, velocity, cup, t::BALL.radius_m, &mut trace);
        let plain = cup::putt_entry(entry, velocity, cup, t::BALL.radius_m, &mut NoProbe);
        match (event, traced, plain) {
            ("capture", cup::PuttEntry::Capture, cup::PuttEntry::Capture)
            | (
                "crossing_to_flagstick",
                cup::PuttEntry::CrossingToFlagstick,
                cup::PuttEntry::CrossingToFlagstick,
            ) => {}
            (
                "lip_out",
                cup::PuttEntry::LipOut { velocity: traced },
                cup::PuttEntry::LipOut { velocity: plain },
            ) => {
                assert_eq!(vector_bits(traced), vector_bits(plain));
                assert_eq!(traced.x, speed * t::CUP_LIP_SPEED_KEEP);
            }
            _ => panic!("unexpected cup result {traced:?}, {plain:?}"),
        }
        let phase = trace.phases().last().unwrap();
        assert_eq!(phase.phase, Phase::Cup);
        assert!(phase.values.iter().any(|value| value.name == event));
        assert!(
            phase
                .values
                .iter()
                .any(|value| value.name == "rim_speed" && value.value == speed)
        );
    }
}
