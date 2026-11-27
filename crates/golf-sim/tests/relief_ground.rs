use std::path::Path;

use golf_sim_base::variant::{Dict, Variant};
use golf_sim_testkit::corpus::FixtureRun;

#[test]
fn relief_ground_matches_reference() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/rules/relief/fixture.bin");
    let mut world_cases = 0;
    let fixture = FixtureRun::read(&path, Some("rules"), &[]).unwrap();
    let is_world_case = |case: &Dict| {
        let call = case.get("call").and_then(Variant::as_str).unwrap();
        call.starts_with("relief_ground.")
    };
    let expected_world_cases = fixture.cases().filter(|case| is_world_case(case)).count();
    let report = fixture
        .check_observing(|case| {
            world_cases += usize::from(is_world_case(case));
        })
        .unwrap();
    assert_eq!(world_cases, expected_world_cases);
    assert!(report.discrete.is_empty(), "{report}");
    print!("{report}");
}
