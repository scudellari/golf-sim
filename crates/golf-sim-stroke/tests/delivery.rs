use std::path::{Path, PathBuf};

use golf_sim_base::{
    ClubId, Real, Seed, Surface, Vec3, corpus,
    num::deg_to_rad,
    trace::{NoProbe, Phase, Source, Trace},
    variant::{Dict, Variant},
};
use golf_sim_physics::{
    facade::{standard_ball, standard_club},
    plane::{ConstantAir, PlaneGround},
    samplers::{AirSample, AirSampler, GroundSample, GroundSampler},
    values::{Contact, Delivery, Launch, Lie, ShotRequest},
};
use golf_sim_stroke::{
    address::{Address, Assists, Intent, Preset, Setting},
    bag::{self, Group},
    delivery::{self},
    tuning,
    wrappers::{self, FlightAir, MownGround, StrikeQuality},
};
use golf_sim_swing::{
    hands::{Swing, exact},
    spec::{StrikeMode, SwingSpec},
};
use golf_sim_testkit::corpus as fixture_corpus;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/stroke/delivery/fixture.bin")
}

#[test]
fn every_recorded_delivery_matches() {
    let report = fixture_corpus::FixtureRun::read(&fixture(), None, &[])
        .unwrap()
        .check()
        .unwrap();
    assert_eq!(report.checked, 682);
    assert!(report.discrete.is_empty());
    assert!(report.skipped.is_empty());
    print!("{report}");
}

#[test]
fn recorded_seed_replays_every_draw_in_order() {
    // Recorded timeout. These draws were read from the reference implementation's two streams.
    let expected = [
        ("timeout.sign", 1.0),
        ("timeout.fault", 1.966_147_808_230_951),
        ("timeout.power", 0.900_977_112_474_414_1),
        ("timeout.toe", 0.140_819_404_546_482_65),
        ("hidden.face_deg", -0.894_996_989_343_383_3),
        ("hidden.path_deg", 1.617_746_455_771_500_2),
        ("hidden.loft_deg", 0.305_770_146_259_750_9),
        ("hidden.speed", 0.417_926_380_452_066_04),
        ("hidden.lp_m", -0.232_177_316_638_698_17),
        ("hidden.depth_m", -0.212_311_500_477_265_19),
        ("hidden.toe_m", 0.949_689_580_852_720_9),
        ("hidden.up_m", 0.369_511_568_598_192_34),
        ("spread.face_deg", -0.607_417_638_067_764_4),
        ("spread.path_deg", 0.382_717_604_915_321_25),
        ("spread.speed", 1.789_262_235_368_285_9),
        ("spread.toe_m", 0.176_667_857_235_998_9),
        ("spread.lp_m", 0.050_792_075_174_010_82),
    ];
    let corpus = corpus::read(&fixture()).unwrap();
    let Variant::Array(cases) = corpus.as_dict().unwrap().get("cases").unwrap() else {
        panic!("expected cases");
    };
    let (input, execution, noise) = cases
        .items
        .iter()
        .map(|case| fixture_corpus::stroke::delivery::read_case(case.as_dict().unwrap()))
        .find(|(input, execution, noise)| {
            input.attempt_seed == Seed(3) && execution.timed_out && *noise
        })
        .expect("recorded timeout for seed 3");

    let mut trace = Trace::default();
    let traced = delivery::build(&input, execution, noise, &mut trace);
    assert_eq!(
        traced,
        delivery::build(&input, execution, noise, &mut NoProbe)
    );
    assert_eq!(trace.phases()[0].phase, Phase::Delivery);
    let draws: Vec<_> = trace.phases()[0]
        .values
        .iter()
        .filter_map(|value| match &value.source {
            Source::SeedDraw { name } => Some((name.as_str(), value.value)),
            _ => None,
        })
        .collect();
    assert_eq!(draws.len(), expected.len());
    for ((name, actual), (expected_name, expected)) in draws.iter().zip(expected) {
        assert_eq!(*name, expected_name);
        assert!(
            (actual - expected).abs() <= 1e-12,
            "{name}: {actual} vs {expected}"
        );
    }
    println!(
        "seed {}: {} draws matched in order",
        input.attempt_seed.0,
        draws.len()
    );
}

#[test]
fn intent_and_presets_use_the_clubs_address_ranges() {
    let stock = Address::stock(ClubId::Iron7);
    let intent = Intent::new(3, -3, 0.15);
    let shaped = Address::from_intent(ClubId::Iron7, intent, None);
    assert_eq!((intent.shape(), intent.height()), (2, -2));
    assert_eq!(shaped.stance_open_rad, deg_to_rad(6.0));
    assert_eq!(shaped.face_open_rad, -deg_to_rad(6.0) * 0.33);
    assert_eq!(
        shaped.ball_forward_m,
        stock.ball_forward_m - 2.0 * tuning::BALL_WIDTH_M
    );
    assert_eq!(shaped.hands_forward_rad, deg_to_rad(8.0));
    assert_eq!(shaped.swing_length, 0.25);
    assert_eq!(shaped.clamped, [Setting::SwingLength]);

    let putter = Address::from_intent(ClubId::Putter, intent, None);
    assert!(putter.same_as(&Address::stock(ClubId::Putter).with_value(Setting::SwingLength, 0.15)));
    let preset = Preset::new("Approach", Group::Irons, shaped);
    assert!(preset.address.clamped.is_empty());
    assert_eq!(Preset::from_dict(&preset.to_dict()), Some(preset));

    let [finder, _, _, chip, ..] = Preset::starters();
    assert_eq!(finder.group, Group::Woods);
    assert_eq!(chip.group, Group::Wedges);
    let putter = Address::from_preset(ClubId::Putter, &finder);
    assert_eq!(putter.tee_height_m, 0.0);
    assert_eq!(putter.clamped, [Setting::TeeHeight]);
    assert!(finder.address.tee_height_m > 0.0);
}

#[test]
fn address_defaults_and_bag_metadata() {
    let dict = Dict::new(vec![
        ("stance_open_rad".into(), Real::NAN.into()),
        ("stance".into(), 1.0.into()),
        ("hands_forward_rad".into(), 0_i64.into()),
        ("tee_height_m".into(), "high".into()),
    ]);
    assert_eq!(Address::from_dict(&dict), Address::default());
    assert_eq!(
        Assists::default(),
        Assists {
            window_scale: 1.0,
            strike_mode: StrikeMode::Reticle
        }
    );
    assert_eq!(tuning::VERSION, "0.1.8");
    assert_eq!(bag::ids().len(), 14);
    assert_eq!(bag::step(ClubId::Putter, i32::MAX), ClubId::Putter);
    assert_eq!(bag::step(ClubId::Driver, -1), ClubId::Driver);
    assert_eq!(bag::step(ClubId::Iron9, 1), ClubId::PitchingWedge);
    assert_eq!(bag::label(ClubId::PitchingWedge), "Pitching Wedge");
    assert_eq!(bag::short_label(ClubId::PitchingWedge), "PW");
    assert_eq!(bag::group(ClubId::PitchingWedge).label(), "Wedges");
}

#[test]
fn samplers_strip_grass_and_count_mean_wind() {
    struct Wind;
    impl AirSampler for Wind {
        fn sample_air(&self, time_s: Real, position: Vec3) -> AirSample {
            AirSample {
                mean_wind_mps: Vec3::new(position.x, 0.0, time_s),
                gust_mps: Vec3::new(0.0, 0.0, 20.0),
                ..AirSample::default()
            }
        }
    }

    let ground = PlaneGround::new(
        GroundSample {
            surface: Surface::Rough,
            grass_height_m: 0.05,
            grass_density: 0.6,
            ..GroundSample::default()
        },
        Vec3::new(0.0, 3.0, 0.0),
    );
    let mown = MownGround(&ground);
    assert_eq!(
        mown.sample_ground(2.0, 3.0),
        GroundSample {
            grass_height_m: 0.0,
            grass_density: 0.0,
            ..ground.sample_ground(2.0, 3.0)
        }
    );

    let air = FlightAir::new(&Wind);
    assert_eq!(air.mean_wind_mps(), Vec3::ZERO);
    for (time, spot) in [
        (3.0, Vec3::new(1.0, 0.0, 0.0)),
        (5.0, Vec3::new(5.0, 0.0, 0.0)),
    ] {
        assert_eq!(air.sample_air(time, spot), Wind.sample_air(time, spot));
    }
    assert_eq!(air.mean_wind_mps(), Vec3::new(3.0, 0.0, 4.0));
}

#[test]
fn strike_reading_distinguishes_splash_and_fat_contact() {
    let mut request = ShotRequest::new(
        standard_club(ClubId::SandWedge),
        standard_ball(),
        Delivery {
            sole_height_m: -0.02,
            ..Delivery::default()
        },
        Lie {
            surface: Surface::Bunker,
            ball_height_m: -0.012,
            ..Lie::default()
        },
    );
    let air = ConstantAir::new(AirSample::default());
    let ground = PlaneGround::new(
        GroundSample {
            surface: Surface::Bunker,
            ..GroundSample::default()
        },
        Vec3::ZERO,
    );
    let mut launch = Launch {
        contact: Contact::Sand,
        turf_entry_behind_m: 0.04,
        strike_up_m: -0.0055,
        interference: 0.4,
        ..Launch::default()
    };
    let execution = exact(
        &SwingSpec::default(),
        Swing::Exact {
            power: 0.0,
            timing_fault: 0.0,
            strike_m: None,
        },
        0,
    );
    assert!(!wrappers::fat(&request, &launch, &air, &ground));
    assert_eq!(
        wrappers::quality(ClubId::SandWedge, execution, &launch, false),
        StrikeQuality::Solid
    );
    launch.contact = Contact::Face;
    assert!(wrappers::fat(&request, &launch, &air, &ground));
    assert_eq!(
        wrappers::quality(ClubId::SandWedge, execution, &launch, true),
        StrikeQuality::Mishit
    );
    launch.interference = 0.0;
    assert_eq!(
        wrappers::quality(ClubId::SandWedge, execution, &launch, false),
        StrikeQuality::Pure
    );
    request.delivery.sole_height_m = -0.012;
    assert!(!wrappers::fat(&request, &launch, &air, &ground));
}
