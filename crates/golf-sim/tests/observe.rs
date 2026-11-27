use std::path::Path;

use golf_sim::{
    action::{Action, ReliefOption, StrokeChoice, Swing},
    hole::Hole,
    setup::{HoleSetup, Mode},
};
use golf_sim_base::{ClubId, Seed, Vec3};
use golf_sim_course::course::Course;
use golf_sim_testkit::corpus::{FixtureRun, hole::assert_last_stroke};

#[test]
fn reference_observation() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/hole/observe/fixture.bin");
    let report = FixtureRun::read(&path, Some("hole"), &[])
        .unwrap()
        .check()
        .unwrap();
    assert!(report.checked > 0);
    assert!(
        report.discrete.is_empty() && report.skipped.is_empty(),
        "{report}"
    );
    print!("{report}");
}

#[test]
fn requested_mode_changes_the_readings_and_leaves_the_hole_replayable() {
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata");
    let course = Course::open(&data.join("courses/test-course")).unwrap();
    let mut setup = HoleSetup::standard(&course, 1, Seed(1));
    setup.settings.info_mode = Mode::Tour;
    let mut hole = Hole::start(course.clone(), setup).unwrap();
    let tee = hole.observe(Mode::Caddie);
    let address = tee.address.unwrap();
    assert!(address.preview.is_some());
    assert!(hole.observe(Mode::Tour).address.unwrap().preview.is_none());
    assert!(tee.last_stroke.is_none());

    let aim = Vec3::new(address.aim[0], address.aim[1], address.aim[2]);
    let choice = StrokeChoice::stock(ClubId::Driver, aim)
        .with_aim_direction(aim)
        .with_swing(Swing::Exact {
            power: 1.0,
            timing_fault: 0.0,
            strike_m: None,
        });
    let step = hole.act(Action::Stroke(choice)).unwrap();
    let bytes = hole.record().to_bytes().unwrap();
    let tour = hole.observe(Mode::Tour);
    let caddie = hole.observe(Mode::Caddie);
    assert_last_stroke(tour.last_stroke.as_ref(), step.stroke.as_ref(), "drive");
    assert_eq!(caddie.last_stroke, tour.last_stroke);
    assert_eq!(tour.score.strokes, 1);
    assert_eq!(tour.to_flag, caddie.to_flag);
    assert!(tour.relief_offers.is_empty());
    assert!(caddie.relief_offers.iter().any(|offer| {
        offer.option == ReliefOption::StrokeAndDistance && offer.penalty_strokes == 1
    }));
    assert_eq!(hole.record().to_bytes().unwrap(), bytes);
    assert_eq!(
        Hole::resume(course, &bytes).unwrap().observe(Mode::Tour),
        tour
    );
}
