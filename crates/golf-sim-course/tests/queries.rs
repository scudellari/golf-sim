use std::path::{Path, PathBuf};

use golf_sim_base::Vec3;
use golf_sim_course::course::Course;
use golf_sim_physics::{samplers::ZoneCrossing, values::Zone};
use golf_sim_testkit::corpus;

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

#[test]
fn course_queries_match_reference() {
    let path = testdata().join("course/queries/fixture.bin");
    let report = corpus::FixtureRun::read(&path, None, &[])
        .unwrap()
        .check()
        .unwrap();
    assert_eq!(report.checked, 13_172);
    assert!(report.discrete.is_empty());
    assert!(report.skipped.is_empty());
    print!("{report}");
    println!("Zone, legal area and obstacle classifications matched exactly.");
}

#[test]
fn legal_boundary_counts_touching_as_in_bounds() {
    let course = Course::open(&testdata().join("courses/test-course")).unwrap();
    let legal = course.legal();
    let obstruction = Vec3::new(189.875, 25.0, 256.0);
    assert!(legal.out_of_bounds(obstruction, 0.124));
    assert!(!legal.out_of_bounds(obstruction, 0.125));
    assert!(!legal.out_of_bounds(Vec3::new(190.0, 25.0, 256.0), 0.0));

    let beyond_property = Vec3::new(516.125, 25.0, 256.0);
    assert!(legal.out_of_bounds(beyond_property, 0.124));
    assert!(!legal.out_of_bounds(beyond_property, 0.125));

    let beside_penalty = Vec3::new(496.125, 25.0, 130.0);
    assert!(legal.penalty_at(beside_penalty, 0.124).is_none());
    assert!(legal.penalty_at(beside_penalty, 0.125).is_some());
}

#[test]
fn crossings_belong_to_the_segment_that_ends_at_the_boundary() {
    let course = Course::open(&testdata().join("courses/test-course")).unwrap();
    let map = course.zone_map();
    let start = Vec3::new(471.0, 0.0, 130.0);
    let edge = Vec3::new(472.0, 0.0, 130.0);
    let end = Vec3::new(497.0, 0.0, 130.0);
    let mut hits = Vec::new();
    map.crossings(start, edge, map.zone_at(start), &mut hits);
    assert_eq!(
        hits,
        [ZoneCrossing {
            fraction: 1.0,
            zone: Zone::PenaltyArea
        }]
    );
    let entered = hits[0].zone;
    map.crossings(edge, end, entered, &mut hits);
    assert_eq!(
        hits,
        [ZoneCrossing {
            fraction: 24.0 / 25.0,
            zone: Zone::InPlay
        }]
    );
    map.crossings(end, end, Zone::InPlay, &mut hits);
    assert!(hits.is_empty());
}

#[test]
fn vertical_ray_hits_ground_and_refuses_origins_below_it() {
    let course = Course::open(&testdata().join("courses/test-course")).unwrap();
    let ground = course.height_at(Vec3::ZERO);
    let origin = Vec3::new(0.0, ground + 2.0, 0.0);
    let direction = Vec3::new(0.0, -3.0, 0.0);
    let hit = course.ray_hit(origin, direction, 5.0).unwrap();
    assert_eq!((hit.x, hit.z), (0.0, 0.0));
    assert!((hit.y - ground).abs() < 1e-4);
    assert!(course.ray_hit(origin, Vec3::ZERO, 5.0).is_none());
    let below_ground = Vec3::new(0.0, ground - 0.5, 0.0);
    assert!(course.ray_hit(below_ground, direction, 5.0).is_none());
}
