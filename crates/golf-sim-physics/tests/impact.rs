use std::path::{Path, PathBuf};

use golf_sim_base::{
    ClubId, Real, Surface, Vec2, Vec3, fm,
    num::{deg_to_rad, rad_to_deg},
    trace::{NoProbe, Phase, Source, Trace},
    variant::Variant,
};
use golf_sim_physics::{
    air::Atmosphere,
    flight, impact,
    plane::PlaneGround,
    roll,
    samplers::{AirSample, GroundSample, GroundSampler},
    tunables as t,
    values::{BallSpec, Delivery, Launch, Lie, ShotRequest, units},
};
use golf_sim_testkit::corpus::{FixtureRun, physics::impact as impact_fixture};
use golf_sim_validate::{
    Conditions, Corpus, Kind, Model, Quantities, Quantity, Status, Submodel, Target, TargetResult,
};

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

#[test]
fn matches_impact_fixture() {
    let path = testdata().join("physics/impact/fixture.bin");
    let mut checked_spin_loft = 0;
    let fixture = FixtureRun::read(&path, None, &[]).unwrap();
    let case_count = fixture.cases().len();
    let report = fixture
        .check_observing(|case| {
            let id = case.get("id").and_then(Variant::as_str).unwrap();
            if impact_fixture::SPIN_LOFT_CASES.contains(&id) {
                checked_spin_loft += 1;
            }
        })
        .unwrap_or_else(|error| {
            if let Ok(fixture) = FixtureRun::read(&path, None, &[])
                && let Ok(drift) = fixture.drift()
            {
                eprint!("{drift}");
            }
            panic!("{error}");
        });
    assert_eq!(report.checked + report.skipped.len(), case_count);
    assert_eq!(checked_spin_loft, impact_fixture::SPIN_LOFT_CASES.len());
    assert_eq!(report.skipped.len(), 34);
    assert!(report.discrete.is_empty());
    print!("{report}");
}

struct DeliveryTarget<'a> {
    target: &'a Target,
    club: ClubId,
    ball: BallSpec,
    air: AirSample,
    ground: PlaneGround,
    lie: Lie,
}

impl<'a> DeliveryTarget<'a> {
    fn new(target: &'a Target, conditions: &Conditions) -> Self {
        assert_eq!(target.kind, Kind::Delivery);
        assert_eq!(conditions.ball, "standard");
        assert_eq!(conditions.wind_mph, 0.0);
        let club = ClubId::from_name(target.inputs["club"].as_str().unwrap()).unwrap();
        let lie = match (club, target.inputs.get("lie").and_then(Quantity::as_str)) {
            (_, Some("bunker")) => Lie {
                surface: Surface::Bunker,
                sand_firmness: target.inputs["sand_firmness"].as_number().unwrap(),
                ..Lie::default()
            },
            (ClubId::Driver, None) => Lie::tee(0.035),
            (ClubId::Putter, None) => Lie {
                surface: Surface::Green,
                ..Lie::default()
            },
            (_, None) => Lie {
                ball_height_m: 0.005,
                ..Lie::default()
            },
            (_, Some(lie)) => panic!("unknown target lie {lie}"),
        };
        let surface = match lie.surface {
            Surface::Bunker | Surface::Green => lie.surface,
            _ => Surface::from_name(&conditions.ground.surface.to_ascii_uppercase()).unwrap(),
        };
        let mut sample = GroundSample {
            surface,
            firmness: conditions.ground.firmness,
            moisture: conditions.ground.moisture,
            ..GroundSample::default()
        };
        if surface == Surface::Green {
            sample.firmness = conditions.green.firmness;
            sample.moisture = conditions.green.moisture;
            sample.green_speed_m = units::feet(conditions.green.stimp_ft);
        }
        let ground = PlaneGround::new(sample, Vec3::ZERO);
        let air = AirSample {
            temperature_c: conditions.air_temp_c,
            pressure_pa: conditions.pressure_pa,
            relative_humidity: conditions.humidity,
            ..AirSample::default()
        };
        Self {
            target,
            club,
            ball: t::standard_ball(),
            air,
            ground,
            lie,
        }
    }

    fn input(&self, key: &str) -> Option<Real> {
        self.target.inputs.get(key).map(|v| v.as_number().unwrap())
    }

    fn launch(&self, loft_deg: Real, toe_m: Real, up_m: Real) -> Launch {
        let club = t::standard_club(self.club);
        let mut delivery = Delivery {
            club_speed_mps: units::mph(self.input("club_mph").unwrap()),
            attack_rad: deg_to_rad(self.input("attack_deg").unwrap()),
            dynamic_loft_rad: deg_to_rad(loft_deg),
            dynamic_lie_rad: deg_to_rad(self.input("dynamic_lie_deg").unwrap_or_default()),
            face_rad: deg_to_rad(self.input("face_deg").unwrap_or_default()),
            path_rad: deg_to_rad(self.input("path_deg").unwrap_or_default()),
            strike_toe_m: toe_m,
            ..Delivery::default()
        };
        if let Some(sole_mm) = self.input("sole_height_mm") {
            delivery.sole_height_m = sole_mm * 0.001;
        } else if self.club == ClubId::Driver || self.target.inputs.contains_key("up_mm") {
            delivery.sole_height_m =
                impact::sole_height_for_strike(&club, &self.ball, &delivery, &self.lie, up_m);
        }
        let request = ShotRequest::new(club, self.ball, delivery, self.lie);
        impact::strike(&request, &self.air, &self.ground, &mut NoProbe)
    }

    fn loft(&self, toe_m: Real, up_m: Real) -> Real {
        if let Some(loft) = self.input("dynamic_loft_deg") {
            return loft;
        }
        let launch_deg = self.input("launch_deg").unwrap();
        let (mut low, mut high) = (0.0, 50.0);
        for _ in 0..30 {
            let mid = (low + high) * 0.5;
            let launch = self.launch(mid, toe_m, up_m);
            if rad_to_deg(launch.readout.launch_angle_rad) < launch_deg {
                low = mid;
            } else {
                high = mid;
            }
        }
        (low + high) * 0.5
    }
}

fn fly(launch: Launch, ball: &BallSpec, sample: &AirSample) -> Vec3 {
    let mut position = launch.position;
    let mut velocity = launch.velocity;
    let mut spin = launch.spin;
    let mut atmosphere = Atmosphere::default();
    let mut time = 0.0;
    while time < t::MAX_SHOT_TIME_S {
        let air = atmosphere.sample(sample, position.y, 0.0).unwrap();
        let step = flight::step(velocity, spin, &air, ball, t::FLIGHT_DT);
        let clearance = position.y - ball.radius_m;
        let next_clearance = clearance + step.travel.y;
        if next_clearance <= 0.0 {
            let fraction = if clearance > 0.0 {
                clearance / (clearance - next_clearance)
            } else {
                0.0
            };
            return position + step.travel * fraction;
        }
        spin = flight::decay_spin(spin, velocity, step.velocity, &air, ball, t::FLIGHT_DT);
        position += step.travel;
        velocity = step.velocity;
        time += t::FLIGHT_DT;
    }
    panic!("flight never reached the ground");
}

fn true_roll_inches(launch: Launch, setup: &DeliveryTarget<'_>) -> Real {
    let ball = &setup.ball;
    let ground = setup
        .ground
        .sample_ground(launch.position.x, launch.position.z);
    let normal_speed = launch.velocity.dot(ground.normal);
    // This putt starts fully supported and is too shallow to lift off the green.
    assert!(launch.position.y - ground.height_m - ball.radius_m <= t::SUPPORTED_LIE_HEIGHT_M);
    assert!((0.0..=fm::sqrt(2.0 * t::GRAVITY * t::MIN_HOP_HEIGHT_M)).contains(&normal_speed));
    let air = Atmosphere::default().sample(&setup.air, 0.0, 0.0).unwrap();
    let air = roll::Air {
        density: air.density_kg_m3,
        viscosity: air.viscosity_pa_s,
        half_area_per_mass: 0.5 * ball.area() / ball.mass_kg,
    };
    let mut state = roll::State {
        velocity: launch.velocity.slide(ground.normal),
        spin: launch.spin,
        motion: roll::Motion::Skid,
    };
    let mut travel = Vec3::ZERO;
    let mut time = 0.0;
    while time < t::MAX_SHOT_TIME_S {
        roll::step(
            &mut state,
            &ground,
            ball,
            &air,
            t::PUTTING_GROUND_DT,
            &mut NoProbe,
        );
        travel += state.velocity * t::PUTTING_GROUND_DT;
        if state.motion == roll::Motion::Roll {
            return travel.length() / units::INCH;
        }
        time += t::PUTTING_GROUND_DT;
    }
    panic!("putt never reached pure roll");
}

const READOUT_NAMES: [&str; 4] = ["ball_mph", "launch_deg", "spin_rpm", "spin_axis_deg"];

fn readout(launch: Launch) -> [Real; 4] {
    [
        units::to_mph(launch.readout.ball_speed_mps),
        rad_to_deg(launch.readout.launch_angle_rad),
        units::to_rpm(launch.readout.spin_rate_rad_s),
        rad_to_deg(launch.readout.spin_axis_rad),
    ]
}

struct ImpactModel;

impl Model for ImpactModel {
    fn predict(&self, target: &Target, conditions: &Conditions) -> Option<Quantities> {
        if target.submodel != Submodel::Impact {
            return None;
        }
        let setup = DeliveryTarget::new(target, conditions);
        let toe_m = setup.input("toe_mm").unwrap_or_default() * 0.001;
        let up_m = setup.input("up_mm").unwrap_or_default() * 0.001;
        let loft = setup.loft(toe_m, up_m);
        let launch = setup.launch(loft, toe_m, up_m);
        let measures = readout(launch);
        let mut quantities = Quantities::from_iter(
            READOUT_NAMES
                .into_iter()
                .zip(measures)
                .map(|(name, v)| (name.into(), Quantity::Number(v))),
        );
        quantities.insert("dynamic_loft_deg".into(), Quantity::Number(loft));
        quantities.insert("speed_ratio".into(), Quantity::Number(launch.smash_factor));
        if target.measured.keys().any(|name| name.ends_with("_change")) {
            let center = readout(setup.launch(loft, 0.0, 0.0));
            for ((name, value), baseline) in READOUT_NAMES.into_iter().zip(measures).zip(center) {
                quantities.insert(format!("{name}_change"), Quantity::Number(value - baseline));
            }
        }
        if target.measured.contains_key("face_share") {
            let face_to_path = setup.input("face_deg").unwrap() - setup.input("path_deg").unwrap();
            let start =
                rad_to_deg(launch.readout.launch_direction_rad) - setup.input("path_deg").unwrap();
            quantities.insert("face_share".into(), Quantity::Number(start / face_to_path));
        }
        if ["carry_m", "carry_yd", "curve_yd"]
            .iter()
            .any(|name| target.measured.contains_key(*name))
        {
            let landing = fly(launch, &setup.ball, &setup.air) - launch.position;
            let carry = Vec2::new(landing.x, landing.z).length();
            let carry_yd = units::to_yards(carry);
            quantities.insert("carry_m".into(), Quantity::Number(carry));
            quantities.insert("carry_yd".into(), Quantity::Number(carry_yd));
            if target.measured.contains_key("curve_yd") {
                let curve = landing.x - carry * fm::tan(launch.readout.launch_direction_rad);
                quantities.insert(
                    "curve_yd".into(),
                    Quantity::Number(conditions.normalize_curve(
                        target,
                        units::to_yards(curve),
                        carry_yd,
                    )),
                );
            }
        }
        if target.measured.contains_key("true_roll_in") {
            quantities.insert(
                "true_roll_in".into(),
                Quantity::Number(true_roll_inches(launch, &setup)),
            );
        }
        Some(quantities)
    }
}

fn check_r47_toe_changes(corpus: &Corpus, result: &mut TargetResult) {
    let setup = corpus
        .files()
        .iter()
        .find_map(|file| {
            let target = file.targets.iter().find(|target| target.id == result.id)?;
            Some(DeliveryTarget::new(target, &file.conditions))
        })
        .unwrap();
    let center = readout(setup.launch(setup.input("dynamic_loft_deg").unwrap(), 0.0, 0.0));
    for (name, baseline) in READOUT_NAMES.into_iter().zip(center) {
        let name = format!("{name}_change");
        let quantity = result.quantities.get_mut(&name).unwrap();
        let earlier_residual = quantity
            .earlier_prediction
            .as_ref()
            .unwrap()
            .as_number()
            .unwrap()
            - quantity.measured.as_number().unwrap();
        let residual = quantity.residual.unwrap();
        let spot = baseline + quantity.predicted.as_ref().unwrap().as_number().unwrap();
        // A difference carries the rounding of both operands, so the allowance scales with their sum.
        let operands = baseline.abs() + spot.abs();
        let allowance = impact_fixture::EIGHT_SINGLE_PRECISION_STEPS * operands.max(1.0);
        let bound = earlier_residual.abs() + allowance;
        println!(
            "{} {name}: residual {residual:.9}, earlier residual {earlier_residual:.9}, cancellation allowance {allowance:.9} at operands {baseline:.9} and {spot:.9}",
            result.id
        );
        quantity.allowed_earlier_residual = Some(bound);
        quantity.status = if residual.abs() <= bound {
            Status::Passed
        } else {
            Status::Failed
        };
    }
    result.status = result
        .quantities
        .values()
        .map(|quantity| quantity.status)
        .max()
        .unwrap();
}

#[test]
fn meets_measured_impact_targets() {
    let corpus = Corpus::load_default(None).unwrap();
    let mut report = corpus.run(&ImpactModel);
    report
        .targets
        .retain(|target| target.submodel == Submodel::Impact);
    let toe = report
        .targets
        .iter_mut()
        .find(|target| target.id == "R47.driver.toe")
        .unwrap();
    check_r47_toe_changes(&corpus, toe);
    print!("{}", report.scorecard());

    let mut failures = Vec::new();
    for target in report.targets {
        for (name, quantity) in target.quantities {
            if (quantity.earlier_prediction.is_some() || quantity.source_tolerance.is_some())
                && quantity.status != Status::Passed
            {
                let earlier_residual = quantity
                    .earlier_prediction
                    .as_ref()
                    .and_then(Quantity::as_number)
                    .zip(quantity.measured.as_number())
                    .map(|(earlier, measured)| earlier - measured);
                failures.push(format!(
                    "{} {name}: residual {:?}, earlier residual {:?}, allowed earlier residual {:?}",
                    target.id, quantity.residual, earlier_residual, quantity.allowed_earlier_residual,
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn traced_impact_records_phases_without_changing_the_launch() {
    let request = ShotRequest::new(
        t::standard_club(ClubId::SandWedge),
        t::standard_ball(),
        Delivery {
            club_speed_mps: units::mph(68.0),
            attack_rad: deg_to_rad(-4.0),
            dynamic_loft_rad: deg_to_rad(46.0),
            sole_height_m: 0.02,
            ..Delivery::default()
        },
        Lie {
            surface: Surface::DeepRough,
            ball_height_m: 0.025,
            grass_height_m: 0.20,
            grass_density: 0.7,
            ..Lie::default()
        },
    );
    let ground = PlaneGround::new(
        GroundSample {
            surface: request.lie.surface,
            ..GroundSample::default()
        },
        Vec3::ZERO,
    );
    let air = AirSample::default();
    let mut trace = Trace::default();
    let traced = impact::strike(&request, &air, &ground, &mut trace);
    let untraced = impact::strike(&request, &air, &ground, &mut NoProbe);
    let bytes =
        |launch| golf_sim_base::variant::encode(&impact_fixture::launch_output(launch)).unwrap();
    assert_eq!(bytes(traced), bytes(untraced));
    assert!(traced.club_speed_at_ball_mps < request.delivery.club_speed_mps);
    assert!(traced.readout.ball_speed_mps > 0.0);
    let [delivery, impact, launch] = trace.phases() else {
        panic!("expected delivery, impact and launch");
    };
    assert_eq!(
        [delivery.phase, impact.phase, launch.phase],
        [Phase::Delivery, Phase::Impact, Phase::Launch]
    );
    assert_eq!(delivery.values[0].value, request.delivery.club_speed_mps);
    assert!(impact.values.iter().any(|entry| {
        entry.value == t::GRASS_ABOVE_HEAD_SHARE
            && matches!(&entry.source, Source::Tunable { name } if name == "GRASS_ABOVE_HEAD_SHARE")
    }));
    assert_eq!(launch.values[0].value, traced.readout.ball_speed_mps);
    println!("delivery, impact and launch traced with identical result bits");
}

#[test]
fn mown_rough_lowers_iron_launch_and_taller_rough_cuts_spin() {
    let air = AirSample::default();
    for (id, mph, attack_deg, loft_deg) in [
        (ClubId::Iron5, 92.0, -3.5, 20.0),
        (ClubId::Iron8, 85.0, -4.5, 27.0),
        (ClubId::SandWedge, 75.0, -5.0, 46.0),
    ] {
        let club = t::standard_club(id);
        let ball = t::standard_ball();
        let delivery = Delivery {
            club_speed_mps: units::mph(mph),
            attack_rad: deg_to_rad(attack_deg),
            dynamic_loft_rad: deg_to_rad(loft_deg),
            ..Delivery::default()
        };
        let fairway = Lie {
            ball_height_m: 0.005,
            ..Lie::default()
        };
        let request = ShotRequest::new(club.clone(), ball, delivery, fairway);
        let clean = impact::strike(&request, &air, &PlaneGround::default(), &mut NoProbe).readout;

        let rough = [0.043, 0.085].map(|grass_height_m| {
            let lie = Lie {
                surface: Surface::Rough,
                ball_height_m: 0.01,
                grass_height_m,
                grass_density: 0.55,
                ..Lie::default()
            };
            let mut delivery = delivery;
            delivery.sole_height_m =
                impact::sole_height_for_strike(&club, &ball, &delivery, &lie, -0.005).max(0.0);
            let request = ShotRequest::new(club.clone(), ball, delivery, lie);
            let ground = PlaneGround::new(
                GroundSample {
                    surface: lie.surface,
                    grass_height_m,
                    grass_density: lie.grass_density,
                    ..GroundSample::default()
                },
                Vec3::ZERO,
            );
            impact::strike(&request, &air, &ground, &mut NoProbe).readout
        });

        let launch_changes =
            rough.map(|shot| rad_to_deg(shot.launch_angle_rad - clean.launch_angle_rad));
        let spins = rough.map(|shot| units::to_rpm(shot.spin_rate_rad_s));
        println!("{id:?} at 43/85 mm: launch changes {launch_changes:.2?}°, spin {spins:.0?} rpm");
        match id {
            ClubId::Iron5 => assert!(launch_changes.iter().all(|&change| change < 0.0)),
            ClubId::SandWedge => assert!(launch_changes.iter().all(|&change| change > 0.0)),
            _ => {}
        }
        assert!(spins[1] < spins[0], "{id:?}: {spins:?}");
    }
}

#[test]
fn fairway_and_tee_match_physics_0_11_1_bits() {
    // Launch velocity and spin from 0.11.1, including cover and a nonzero film
    // in the tall stands so the old friction and restitution both get exercised.
    let cases = [
        (
            Surface::Fairway,
            0.005,
            0.012,
            0.3,
            [
                0xbf97_2a6f_cb8c_fe91,
                0x402c_4721_c51b_3b66,
                0xc048_c28d_63da_6024,
                0x4084_f919_22d5_fc56,
                0x3fea_f00a_1405_b98b,
                0xbfb0_f5f4_8acb_a904,
            ],
        ),
        (
            Surface::Tee,
            0.009,
            0.0,
            0.0,
            [
                0,
                0x402c_4c7d_51fd_9460,
                0xc048_c786_ad87_11ca,
                0x4084_fde2_c168_7d20,
                0,
                0,
            ],
        ),
        (
            Surface::Fairway,
            0.005,
            0.085,
            0.3,
            [
                0xbfe2_dd86_78a5_957e,
                0x4029_7cc2_eac6_d4b1,
                0xc045_daee_1b49_3434,
                0x4081_93f0_a2c1_30f2,
                0x4034_6b34_e896_faa8,
                0xbffa_22f7_3e96_ae28,
            ],
        ),
        (
            Surface::Tee,
            0.009,
            0.085,
            0.3,
            [
                0xbfe2_7a92_18eb_cc2b,
                0x4029_e3a9_d7e5_c05d,
                0xc046_3373_00dd_01de,
                0x4081_db97_d740_8281,
                0x4034_00da_7029_93b2,
                0xbff9_9a9b_e70e_bc3b,
            ],
        ),
    ];
    for (surface, ball_height_m, grass_height_m, grass_density, expected) in cases {
        let lie = Lie {
            surface,
            ball_height_m,
            grass_height_m,
            grass_density,
            ..Lie::default()
        };
        let club = t::standard_club(ClubId::Iron6);
        let ball = t::standard_ball();
        let mut delivery = Delivery {
            club_speed_mps: units::mph(86.0),
            attack_rad: deg_to_rad(-3.3),
            dynamic_loft_rad: deg_to_rad(22.0),
            ..Delivery::default()
        };
        delivery.sole_height_m =
            impact::sole_height_for_strike(&club, &ball, &delivery, &lie, -0.004);
        let request = ShotRequest::new(club, ball, delivery, lie);
        let ground = PlaneGround::new(
            GroundSample {
                surface,
                ..GroundSample::default()
            },
            Vec3::ZERO,
        );
        let launch = impact::strike(&request, &AirSample::default(), &ground, &mut NoProbe);
        assert!(launch.valid);
        let v = launch.velocity;
        let s = launch.spin;
        let actual = [v.x, v.y, v.z, s.x, s.y, s.z].map(Real::to_bits);
        assert_eq!(actual, expected, "{surface:?} at {grass_height_m} m");
    }
}
