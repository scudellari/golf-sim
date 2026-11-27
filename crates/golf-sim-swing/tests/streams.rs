use std::path::{Path, PathBuf};

use golf_sim_base::variant::Variant;
use golf_sim_testkit::corpus::{FixtureRun, swing};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/swing/streams/fixture.bin")
}

#[test]
fn every_recorded_swing_matches() {
    let mut golfer_cases = 0;
    let report = FixtureRun::read(&fixture(), Some("swing"), &[])
        .unwrap()
        .check_observing(|case| {
            let input = case.get("input").unwrap().as_dict().unwrap();
            if input.get("golfer").and_then(Variant::as_str) == Some("hole_kit") {
                golfer_cases += 1;
            }
        })
        .unwrap();
    assert!(report.checked > 0);
    assert!(golfer_cases >= 56);
    assert!(
        report.skipped.is_empty() && report.discrete.is_empty(),
        "{report}"
    );
    print!("{report}");
}

#[test]
fn advance_cadence_preserves_every_execution() {
    let mut checked = 0;
    FixtureRun::read(&fixture(), Some("swing"), &[])
        .unwrap()
        .check_observing(|case| {
            swing::check_cadence(case);
            checked += 1;
        })
        .unwrap();
    assert!(checked > 0);
}
