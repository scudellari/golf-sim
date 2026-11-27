use std::path::{Path, PathBuf};

use golf_sim::{
    action::{Action, ReliefOption, StrokeChoice, Swing},
    hole::Hole,
    records::hole::{Entry, HoleRecord, Refusal, StrokeEntry, SwingPress},
    setup::{HoleSetup, StrikeVariant},
};
use golf_sim_base::{ClubId, Error, Real, Seed, Vec3, trace::Phase};
use golf_sim_course::course::Course;
use golf_sim_stroke::{
    address::{Address, Setting},
    delivery::DeliveryInput,
    stroke,
};
use golf_sim_swing::{hands::Profile, session::SwingSession};
use golf_sim_testkit::corpus::{FixtureRun, hole::assert_replays};

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

#[test]
fn reference_holes() {
    let report = FixtureRun::read(
        &testdata().join("records/holes/fixture.bin"),
        Some("records"),
        &[],
    )
    .unwrap()
    .check()
    .unwrap();
    assert_eq!(report.checked, 30);
    assert!(report.skipped.is_empty());
    print!("{report}");
}

#[test]
fn skill_records_and_traces() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<Hole>();
    let course = Course::open(&testdata().join("courses/test-course")).unwrap();
    let mut records = 0;
    for profile in [Profile::SCRATCH, Profile::HCP15] {
        for seed in 1..=3 {
            let setup = HoleSetup::standard(&course, 1, Seed(seed));
            let tee = setup.tee(&course).unwrap().position;
            let mut hole = Hole::start(course.clone(), setup.clone()).unwrap();
            let mut traced = Hole::start(course.clone(), setup).unwrap().with_traces(2);
            let choice = |power| {
                let mut choice =
                    StrokeChoice::stock(ClubId::Putter, tee + Vec3::new(1.0, 0.0, 0.0))
                        .with_aim_direction(Vec3::new(1.0, 0.0, 0.0))
                        .with_swing(Swing::Skill(profile));
                choice.address = choice.address.with_value(Setting::SwingLength, power);
                Action::Stroke(choice)
            };
            let mut history = Vec::new();
            for power in [0.2, 0.3, 0.4] {
                hole.act(choice(power)).unwrap();
                traced.act(choice(power)).unwrap();
                history.push(traced.traces().back().unwrap().clone());
            }
            assert!(hole.traces().is_empty());
            assert!(traced.traces().iter().eq(&history[1..]));
            for trace in traced.traces() {
                assert!(
                    trace
                        .phases()
                        .iter()
                        .any(|phase| phase.phase == Phase::Delivery)
                );
                assert_eq!(trace.phases().last().unwrap().phase, Phase::Ruling);
            }
            assert_eq!(
                hole.record().to_bytes().unwrap(),
                traced.record().to_bytes().unwrap()
            );
            assert_replays(&course, &traced);
            let bytes = traced.record().to_bytes().unwrap();
            let mut replay = Hole::resume(course.clone(), &bytes).unwrap();
            replay.act(choice(0.25)).unwrap();
            hole.act(choice(0.25)).unwrap();
            assert_eq!(
                replay.record().to_bytes().unwrap(),
                hole.record().to_bytes().unwrap()
            );
            records += 1;
        }
    }
    println!(
        "Replayed {records} skill records byte for byte with identical records with tracing enabled."
    );
}

#[test]
fn replay_names_the_first_changed_result() {
    let course = Course::open(&testdata().join("courses/test-course")).unwrap();
    let setup = HoleSetup::standard(&course, 1, Seed(7));
    let aim = setup.cup(&course).unwrap().position;
    let mut hole = Hole::start(course.clone(), setup).unwrap();
    assert!(hole.act(Action::GoForward).is_err());
    assert!(hole.record().entries.is_empty());
    hole.act(Action::Stroke(
        StrokeChoice::stock(ClubId::Driver, aim).with_swing(Swing::Exact {
            power: 0.5,
            timing_fault: 0.0,
            strike_m: None,
        }),
    ))
    .unwrap();
    let mut record = hole.record().clone();
    let Entry::Stroke(stroke) = &mut record.entries[0] else {
        panic!("expected stroke")
    };
    assert_eq!(stroke.address_usec, Some(250_000));
    assert_eq!(
        stroke.facts.gust_time_s.to_bits(),
        (stroke.facts.execution.impact_usec as Real / 1_000_000.0).to_bits()
    );
    stroke.facts.lie.ball_height_m += 0.001;
    let assert_diverges = |bytes: &[u8], field| {
        assert!(matches!(
            Hole::resume(course.clone(), bytes),
            Err(Refusal::Invalid(Error::Diverged { entry: 0, path })) if path == field
        ));
    };
    let bytes = record.to_bytes().unwrap();
    assert_diverges(&bytes, "lie.ball_height_m");
    let Entry::Stroke(stroke) = &mut record.entries[0] else {
        panic!("expected stroke")
    };
    stroke.facts.execution.timing_fault = -0.0;
    assert_diverges(&record.to_bytes().unwrap(), "execution.timing_fault");
    hole.act(Action::Relief {
        option: ReliefOption::StrokeAndDistance,
        footprint: vec![],
    })
    .unwrap();
    let mut record = hole.record().clone();
    let Entry::Decision(decision) = &mut record.entries[1] else {
        panic!("expected decision")
    };
    *decision.ruling.get_mut("penalty").unwrap() = 4_i64.into();
    assert!(matches!(
        Hole::resume(course, &record.to_bytes().unwrap()),
        Err(Refusal::Invalid(Error::Diverged { entry: 1, path })) if path == "ruling.penalty"
    ));
}

#[test]
fn replay_judges_stamped_presses_again() {
    let course = Course::open(&testdata().join("courses/test-course")).unwrap();
    let mut setup = HoleSetup::standard(&course, 1, Seed(7));
    setup.settings.strike_variant = StrikeVariant::B;
    let mut hole = Hole::start(course.clone(), setup.clone()).unwrap();
    let ball = hole.ball_lie().unwrap();
    let aim = Vec3::new(1.0, 0.0, 0.0);
    let club = ClubId::Driver;
    let address = Address::stock(club);
    let seed = setup.seed.derive("attempt/0");
    let world = setup.world(&course, hole.clock_s()).unwrap();
    let input = stroke::StrokeInput::new(
        DeliveryInput {
            club,
            lie: world.address_lie(ball, &address.footprint(club), aim),
            address,
            attempt_seed: seed,
        },
        aim,
    );
    let spec = stroke::swing_spec(&input, setup.settings.assists());
    let address_usec = 1_000_000;
    let start = address_usec + 250_000;
    let power = start + 550_000;
    let mut session = SwingSession::new(spec, start, stroke::reticle_seed(seed));
    session.press(power);
    let impact = session.close_usec();
    session.press(impact);
    let execution = session.execution().unwrap();
    hole.act(Action::Stroke(
        StrokeChoice::stock(club, ball.spot + aim).with_swing(Swing::Exact {
            power: execution.power,
            timing_fault: execution.timing_fault,
            strike_m: execution.strike_m,
        }),
    ))
    .unwrap();
    let mut record = hole.record().clone();
    record.driver = HoleRecord::console_driver();
    let Entry::Stroke(stroke) = &mut record.entries[0] else {
        panic!("expected stroke")
    };
    stroke.address_usec = Some(address_usec);
    stroke.facts.execution = execution;
    stroke.swing = StrokeEntry::pressed_swing([start, power, impact].map(SwingPress::Press));
    let bytes = record.to_bytes().unwrap();
    let replay = Hole::resume(course, &bytes).unwrap();
    assert_eq!(replay.record().to_bytes().unwrap(), bytes);
}
