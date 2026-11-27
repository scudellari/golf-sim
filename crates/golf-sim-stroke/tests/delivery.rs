use std::{
    convert::Infallible,
    path::{Path, PathBuf},
};

use golf_sim_base::{
    ClubId, Real, Seed, Surface, Vec2, Vec3, corpus,
    digest::object,
    num::deg_to_rad,
    trace::{NoProbe, Phase, Source, Trace},
    variant::{Dict, Variant},
};
use golf_sim_conditions::lies::{AddressLie, BallLie, DivotLie, Sit};
use golf_sim_physics::{
    facade::{standard_ball, standard_club},
    plane::{ConstantAir, PlaneGround},
    samplers::{AirSample, AirSampler, GroundSample, GroundSampler},
    values::{Contact, Delivery, Launch, Lie, ShotRequest},
};
use golf_sim_stroke::{
    address::{Address, Assists, Intent, Preset, Setting},
    bag::{self, Group},
    delivery::{self, BuiltDelivery, DeliveryInput},
    tuning,
    wrappers::{self, FlightAir, MownGround, StrikeQuality},
};
use golf_sim_swing::{
    execution::Execution,
    hands::{Swing, exact},
    spec::{StrikeMode, SwingSpec},
};
use golf_sim_testkit::tolerance;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/stroke/delivery/fixture.bin")
}

fn field<'a>(record: &'a Dict, name: &str) -> &'a Variant {
    record.get(name).unwrap_or_else(|| panic!("missing {name}"))
}

fn dict<'a>(record: &'a Dict, name: &str) -> &'a Dict {
    field(record, name).as_dict().unwrap()
}

fn integer(record: &Dict, name: &str) -> i64 {
    let Variant::Int(value) = field(record, name) else {
        panic!("expected integer at {name}");
    };
    value.value()
}

fn number(record: &Dict, name: &str) -> Real {
    match field(record, name) {
        Variant::Int(value) => value.value() as Real,
        Variant::Float(value) => value.value(),
        _ => panic!("expected number at {name}"),
    }
}

fn boolean(record: &Dict, name: &str) -> bool {
    let Variant::Bool(value) = field(record, name) else {
        panic!("expected bool at {name}");
    };
    *value
}

fn surface(record: &Dict, name: &str) -> Surface {
    Surface::ALL[usize::try_from(integer(record, name)).unwrap()]
}

fn read_case(case: &Dict) -> (DeliveryInput, Execution, bool) {
    assert_eq!(field(case, "call").as_str(), Some("delivery_builder.build"));

    let args = dict(case, "input");
    let input = dict(args, "input");
    let lie = dict(input, "lie");
    let ball = dict(lie, "ball");
    let Variant::Vector3(spot) = field(ball, "spot") else {
        panic!("expected ball spot");
    };
    let [x, y, z] = spot.values;
    let ball = BallLie {
        spot: Vec3::new(x, y, z),
        surface: surface(ball, "surface"),
        sit: [Sit::Up, Sit::Normal, Sit::Down][integer(ball, "sit") as usize],
        ball_height_m: number(ball, "ball_height_m"),
        grass_height_m: number(ball, "grass_height_m"),
        grass_density: number(ball, "grass_density"),
        sand_firmness: number(ball, "sand_firmness"),
        divot: [
            DivotLie::None,
            DivotLie::Fresh,
            DivotLie::Old,
            DivotLie::SandFilled,
        ][integer(ball, "divot") as usize],
    };
    let input = DeliveryInput {
        club: ClubId::from_name(field(input, "club_id").as_str().unwrap()).unwrap(),
        address: Address::from_dict(dict(input, "address")),
        lie: AddressLie {
            ball,
            slope_along_rad: number(lie, "slope_along_rad"),
            slope_across_rad: number(lie, "slope_across_rad"),
            ball_slope_along_rad: number(lie, "ball_slope_along_rad"),
            lead_foot_surface: surface(lie, "lead_foot_surface"),
            trail_foot_surface: surface(lie, "trail_foot_surface"),
            club_entry_surface: surface(lie, "club_entry_surface"),
        },
        attempt_seed: Seed(integer(input, "attempt_seed")),
    };
    let execution = dict(args, "execution");
    let Variant::Vector2(strike) = field(execution, "strike_m") else {
        panic!("expected strike on the face");
    };
    let [toe, up] = strike.values;
    let execution = Execution {
        power: number(execution, "power"),
        timing_fault: number(execution, "timing_fault"),
        timed_out: boolean(execution, "timed_out"),
        strike_m: boolean(execution, "has_strike").then_some(Vec2::new(toe, up)),
        timing_error_s: 0.0,
        impact_usec: 0,
        backswing_s: 0.0,
        ring_s: 0.0,
    };
    (input, execution, boolean(args, "with_noise"))
}

macro_rules! record {
    ($($key:ident: $value:expr),+ $(,)?) => {
        object([$( (stringify!($key), ($value).into()) ),+])
    };
}

fn output(built: &BuiltDelivery) -> Variant {
    let BuiltDelivery {
        club,
        delivery,
        lie,
        fault,
    } = built;
    record! {
        club: record! {
            id: club.id.as_str(),
            category: club.category as i64,
            loft_rad: club.loft_rad,
            length_m: club.length_m,
            head_mass_kg: club.head_mass_kg,
            cor: club.cor,
            cor_speed_slope_delta: club.cor_speed_slope_delta,
            cor_falloff_toe: club.cor_falloff_toe,
            cor_falloff_up: club.cor_falloff_up,
            cg_depth_m: club.cg_depth_m,
            cg_up_m: club.cg_up_m,
            cg_toe_m: club.cg_toe_m,
            moi_vertical: club.moi_vertical,
            moi_heel_toe: club.moi_heel_toe,
            moi_forward: club.moi_forward,
            bulge_radius_m: club.bulge_radius_m,
            roll_radius_m: club.roll_radius_m,
            face_center_height_m: club.face_center_height_m,
            face_height_m: club.face_height_m,
            face_half_width_m: club.face_half_width_m,
            face_friction: club.face_friction,
            tangential_restitution_up: club.tangential_restitution_up,
            tangential_restitution_toe: club.tangential_restitution_toe,
            tangential_restitution_speed_slope: club.tangential_restitution_speed_slope,
            arc_radius_m: club.arc_radius_m,
            sole_width_m: club.sole_width_m,
            sole_bounce_rad: club.sole_bounce_rad,
        },
        delivery: record! {
            club_speed_mps: delivery.club_speed_mps,
            path_rad: delivery.path_rad,
            attack_rad: delivery.attack_rad,
            face_rad: delivery.face_rad,
            dynamic_loft_rad: delivery.dynamic_loft_rad,
            dynamic_lie_rad: delivery.dynamic_lie_rad,
            strike_toe_m: delivery.strike_toe_m,
            sole_height_m: delivery.sole_height_m,
        },
        lie: record! {
            surface: lie.surface as i64,
            ball_height_m: lie.ball_height_m,
            grass_height_m: lie.grass_height_m,
            grass_density: lie.grass_density,
            sand_firmness: lie.sand_firmness,
        },
        fault: *fault,
    }
}

#[test]
fn every_recorded_delivery_matches() {
    let report = tolerance::check(&fixture(), "stroke", |case| {
        let (input, execution, noise) = read_case(case);
        Ok::<_, Infallible>(output(&delivery::build(
            &input,
            execution,
            noise,
            &mut NoProbe,
        )))
    })
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
    let Variant::Array(cases) = field(corpus.as_dict().unwrap(), "cases") else {
        panic!("expected cases");
    };
    let (input, execution, noise) = cases
        .items
        .iter()
        .map(|case| read_case(case.as_dict().unwrap()))
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
