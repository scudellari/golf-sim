use std::path::Path;

use golf_sim_base::{
    Real, Seed, Surface, Vec2, Vec3,
    container::Manifest,
    seed::SeedStream,
    trace::{NoProbe, Phase, Source, Trace},
    variant::{self, Variant},
};
use golf_sim_conditions::{
    grounds::CourseGround,
    lies::{BallLie, Sit},
    world::World,
};
use golf_sim_course::{course::Course, legal::StanceFootprint};
use golf_sim_physics::{
    plane::{ConstantAir, PlaneGround},
    samplers::{AirSampler, GroundSampler},
};
use golf_sim_testkit::corpus as fixture_corpus;

fn testdata() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

#[test]
fn matches_world_fixture() {
    let path = testdata().join("world/constant/fixture.bin");
    let fixture = fixture_corpus::FixtureRun::read(&path, None, &[]).unwrap();
    let case_count = fixture.cases().len();
    let report = fixture.check().unwrap();
    print!("{report}");
    assert_eq!(report.checked, case_count);
    assert!(report.discrete.is_empty());
    assert!(report.skipped.is_empty());
}

#[test]
fn course_settle_traces_the_draw_and_turf() {
    let course = Course::open(&testdata().join("courses/test-course")).unwrap();
    let world = World::on_course(
        CourseGround::new(&course, 0.5, 0.3, 2.85),
        ConstantAir::default(),
        Some(Vec3::new(414.0, 25.83, 74.0)),
        32400.0,
    );
    let spot = Vec3::new(350.2198791503906, 99.0, 62.7130012512207);
    let seed = Seed(-1545875291267850878);
    let mut trace = Trace::default();
    let ball = world.settle(spot, seed, &mut trace);

    assert!(world.legal().is_some());
    assert_eq!(
        world.cup(true).unwrap().position,
        world.cup_position().unwrap()
    );
    assert!(world.cup(true).unwrap().flagstick_in);
    assert_eq!(ball.surface, Surface::Fairway);
    assert_eq!(ball.sit, Sit::Normal);
    assert_eq!(trace.phases().len(), 1);
    let phase = &trace.phases()[0];
    assert_eq!(phase.phase, Phase::Lie);
    let draw = phase.values.iter().find(|v| v.name == "sit_draw").unwrap();
    assert_eq!(
        draw.value.to_bits(),
        SeedStream::new(seed).next_float().to_bits()
    );
    assert!(matches!(draw.source, Source::SeedDraw { .. }));
    let turf = phase
        .values
        .iter()
        .find(|v| v.name == "grass_height_m")
        .unwrap();
    assert_eq!(turf.value.to_bits(), ball.grass_height_m.to_bits());
    assert!(matches!(turf.source, Source::Turf { .. }));
}

fn course_with_deep_rough(height_m: Real) -> std::sync::Arc<Course> {
    let dir = testdata().join("courses/test-course");
    let course = Course::open(&dir).unwrap();
    let index = usize::from(course.material_index_at(Vec2::ZERO).unwrap().0);
    let bytes = Manifest::read(&dir).unwrap().read_payload(&dir).unwrap();
    let Variant::Dict(mut payload) = variant::decode(&bytes).unwrap() else {
        panic!("expected a course dictionary");
    };
    let Variant::Array(materials) = payload.get_mut("materials").unwrap() else {
        panic!("expected a material table");
    };
    let Variant::Dict(material) = &mut materials.items[index] else {
        panic!("expected a material dictionary");
    };
    *material.get_mut("kind").unwrap() = "DEEP_ROUGH".into();
    *material.get_mut("grass_height_m").unwrap() = height_m.into();
    let Variant::Dict(property) = payload.get_mut("property").unwrap() else {
        panic!("expected a property dictionary");
    };
    let Variant::Dict(surface) = property.get_mut("surface").unwrap() else {
        panic!("expected a surface dictionary");
    };
    if let Some(stand) = surface.get_mut("stand") {
        *stand = Variant::PackedByteArray(Vec::new());
    }
    Course::parse(&variant::encode(&Variant::Dict(payload)).unwrap()).unwrap()
}

#[test]
fn settling_on_course_fescue_uses_the_height_of_its_stand() {
    let course = course_with_deep_rough(0.42);
    let ground = CourseGround::new(&course, 0.5, 0.3, 2.85);
    assert_eq!(ground.sample_ground(0.0, 0.0).grass_height_m, 0.42);
    let world = World::on_course(ground, ConstantAir::default(), None, 0.0);
    let lie = world.settle(Vec3::ZERO, Seed(1), &mut NoProbe);
    assert_eq!(lie.surface, Surface::DeepRough);
    assert_eq!(lie.grass_height_m, 0.42);
    assert!((lie.grass_density - 0.55 * 0.225 / 0.42).abs() < 1e-12);
}

#[test]
fn practice_keeps_the_chosen_lie_and_reads_gusts_on_its_clock() {
    let mut air = ConstantAir::default();
    let gust = Vec3::new(3.0, 0.0, 0.0);
    air.add_gust(31.0, 33.0, gust);
    let fixed = BallLie::on(Surface::Tee, Sit::Normal, Vec3::ZERO);
    let world = World::practice(PlaneGround::default(), air, fixed, None, 30.0);

    let mut trace = Trace::default();
    let spot = Vec3::new(1.0, 15.0, 2.0);
    let ball = world.settle(spot, Seed(7), &mut trace);
    assert_eq!(
        ball,
        BallLie {
            spot: Vec3::new(1.0, 0.0, 2.0),
            ..fixed
        }
    );
    assert!(
        trace.phases()[0]
            .values
            .iter()
            .all(|v| !matches!(v.source, Source::SeedDraw { .. }))
    );
    assert!(!world.has_cup());
    assert!(world.legal().is_none());
    assert_eq!(world.wind_readout(spot, 1.0).gust_mps, gust);
    assert_eq!(world.wind_readout(spot, 3.0).gust_mps, Vec3::ZERO);
    assert_eq!(
        world.air_mean(spot).sample_air(31.0, spot).gust_mps,
        Vec3::ZERO
    );

    let address = world.address_lie(ball, &StanceFootprint::default(), Vec3::new(0.0, 0.0, -1.0));
    assert_eq!(address.club_entry_surface, Surface::Fairway);
    assert_eq!(address.sim_lie(0.04).ball_height_m, 0.04);
}
