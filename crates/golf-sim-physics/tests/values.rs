use std::path::{Path, PathBuf};

use golf_sim_base::{
    Vec2, Vec3,
    variant::{Dict, Variant},
};
use golf_sim_physics::{
    plane::PlaneGround,
    samplers::{GroundSampler, ZoneCrossing},
    tunables::{self as t},
    values::Zone,
};
use golf_sim_testkit::corpus::FixtureRun;

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/physics/values/fixture.bin")
}

fn dict(value: &Variant) -> &Dict {
    value.as_dict().expect("expected dictionary")
}

fn field<'a>(record: &'a Dict, key: &str) -> &'a Variant {
    record
        .get(key)
        .unwrap_or_else(|| panic!("missing field {key}"))
}

#[test]
fn values_match_physics_fixture() {
    let mut ground_cases = 0;
    let mut playback_cases = 0;
    let fixture = FixtureRun::read(&fixture_path(), None, &[]).unwrap();
    let case_count = fixture.cases().len();
    let report = fixture
        .check_observing(|case| match field(case, "call").as_str().unwrap() {
            "sim_plane_ground.height_at" | "sim_plane_ground.sample_ground" => ground_cases += 1,
            "sim_shot_result.position_at" => playback_cases += 1,
            "tunables.constants" => {
                let expected = field(case, "output");
                let count = dict(expected).entries().len();
                assert_eq!(count, 159);
                println!(
                    "checking {count} recorded tunables against model {}",
                    t::VERSION
                );
            }
            _ => {}
        })
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(report.checked, case_count);
    assert!(report.skipped.is_empty());
    assert_eq!(report.field_departures.len(), 19);
    print!("{report}");
    println!(
        "{ground_cases} plane heights and samples and {playback_cases} position_at cases passed the physics tolerance"
    );
}

#[test]
fn crossings_respect_zone_priority_and_include_the_segment_end() {
    let mut ground = PlaneGround::default();
    ground
        .add_zone(Vec2::new(1.0, -1.0), Vec2::new(5.0, 1.0), Zone::PenaltyArea)
        .add_zone(Vec2::new(2.0, -1.0), Vec2::new(4.0, 1.0), Zone::OutOfBounds);
    let end = Vec3::new(5.0, 10.0, 0.0);
    let mut crossings = Vec::new();
    ground.crossings(Vec3::ZERO, end, Zone::InPlay, &mut crossings);
    assert_eq!(
        crossings,
        [
            ZoneCrossing {
                fraction: 0.2,
                zone: Zone::PenaltyArea
            },
            ZoneCrossing {
                fraction: 0.4,
                zone: Zone::OutOfBounds
            },
            ZoneCrossing {
                fraction: 0.8,
                zone: Zone::PenaltyArea
            },
            ZoneCrossing {
                fraction: 1.0,
                zone: Zone::InPlay
            },
        ]
    );
    ground.crossings(end, Vec3::new(6.0, 0.0, 0.0), Zone::InPlay, &mut crossings);
    assert!(crossings.is_empty());
}
