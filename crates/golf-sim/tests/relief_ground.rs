use std::path::Path;

use golf_sim_base::variant::Variant;
use golf_sim_testkit::corpus::FixtureRun;

#[test]
fn relief_ground_matches_reference() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/rules/relief/fixture.bin");
    let mut world_cases = 0;
    let report = FixtureRun::read(&path, Some("rules"), &[])
        .unwrap()
        .check_observing(|case| {
            let call = case.get("call").and_then(Variant::as_str).unwrap();
            world_cases += usize::from(call.starts_with("relief_ground."));
        })
        .unwrap();
    assert!(world_cases >= 23);
    assert!(report.discrete.is_empty(), "{report}");
    print!("{report}");
}
