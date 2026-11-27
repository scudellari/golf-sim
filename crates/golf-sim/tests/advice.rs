use std::path::Path;

use golf_sim::{
    action::{StrokeChoice, Swing},
    hole::Hole,
    setup::HoleSetup,
};
use golf_sim_base::{ClubId, Seed, Vec3};
use golf_sim_course::course::Course;
use golf_sim_stroke::address::Setting;
use golf_sim_testkit::corpus::FixtureRun;

#[test]
fn preview_uses_the_address_and_target() {
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata");
    let course = Course::open(&data.join("courses/test-course")).unwrap();
    let setup = HoleSetup::standard(&course, 1, Seed(1));
    let hole = Hole::start(course, setup).unwrap();
    let ball = hole.ball_lie().unwrap();
    let record = hole.record().to_bytes().unwrap();
    let target = ball.spot + Vec3::new(40.0, 0.0, -30.0);
    let choice = StrokeChoice::stock(ClubId::Iron7, target);
    let preview = hole.preview(&choice).unwrap();
    assert!(preview.carry_m.is_finite());
    assert_eq!(preview.target_distance_m, 50.0);

    let mut choice = choice.with_swing(Swing::Exact {
        power: 0.4,
        timing_fault: 0.7,
        strike_m: None,
    });
    let same_preview = hole.preview(&choice).unwrap();
    assert_eq!(preview.carry_m, same_preview.carry_m);
    assert_eq!(preview.carry_to_target_m, same_preview.carry_to_target_m);

    choice.address = choice.address.with_value(Setting::SwingLength, 0.6);
    let shorter = hole.preview(&choice).unwrap();
    assert!(shorter.carry_m < preview.carry_m);
    assert!(shorter.has_hint);
    assert_eq!(hole.ball_lie(), Some(ball));
    assert_eq!(hole.record().to_bytes().unwrap(), record);
}

#[test]
fn reference_advice_and_actions() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/hole/advice/fixture.bin");
    let fixture = FixtureRun::read(&path, Some("hole"), &[]).unwrap();
    let case_count = fixture.cases().len();
    let report = fixture.check().unwrap();
    assert_eq!(report.checked + report.skipped.len(), case_count);
    assert_eq!(report.skipped.len(), 19);
    assert!(report.discrete.is_empty(), "{report}");
    print!("{report}");
}
