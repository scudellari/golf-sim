use std::path::{Path, PathBuf};

use golf_sim_base::variant::{Dict, Variant};
use golf_sim_testkit::corpus::{FixtureRun, swing};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/swing/streams/fixture.bin")
}

#[test]
fn every_recorded_swing_matches() {
    let fixture = FixtureRun::read(&fixture(), Some("swing"), &[]).unwrap();
    let case_count = fixture.cases().len();
    let is_golfer = |case: &Dict| {
        let input = case.get("input").unwrap().as_dict().unwrap();
        input.get("golfer").and_then(Variant::as_str) == Some("hole_kit")
    };
    let expected_golfer_cases = fixture.cases().filter(|case| is_golfer(case)).count();
    let mut golfer_cases = 0;
    let report = fixture
        .check_observing(|case| {
            golfer_cases += usize::from(is_golfer(case));
        })
        .unwrap();
    assert_eq!(report.checked, case_count);
    assert_eq!(golfer_cases, expected_golfer_cases);
    assert!(
        report.skipped.is_empty() && report.discrete.is_empty(),
        "{report}"
    );
    print!("{report}");
}

#[test]
fn advance_cadence_preserves_every_execution() {
    let fixture = FixtureRun::read(&fixture(), Some("swing"), &[]).unwrap();
    let case_count = fixture.cases().len();
    let mut checked = 0;
    fixture
        .check_observing(|case| {
            swing::check_cadence(case);
            checked += 1;
        })
        .unwrap();
    assert_eq!(checked, case_count);
}
