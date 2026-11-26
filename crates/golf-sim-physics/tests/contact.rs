use std::path::{Path, PathBuf};

use golf_sim_base::{
    Real, Surface, Vec3,
    trace::{NoProbe, Phase, Source, Trace},
    variant::{Components, Dict, FloatWidth, Variant},
};
use golf_sim_physics::{
    bounce, cup,
    roll::{self, Motion},
    samplers::GroundSample,
    sweep, tunables as t,
    values::{BallSpec, Cup, ObstacleHit, ObstacleKind, units},
};
use golf_sim_testkit::tolerance;
use golf_sim_validate::{
    Conditions, Corpus, EarlierVia, Kind, Model, Quantities, Quantity, Status, Submodel, Target,
};

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

fn dict(value: &Variant) -> &Dict {
    value.as_dict().expect("expected dictionary")
}

fn field<'a>(record: &'a Dict, key: &str) -> &'a Variant {
    record.get(key).unwrap_or_else(|| panic!("missing {key}"))
}

fn number(record: &Dict, key: &str) -> Real {
    match field(record, key) {
        Variant::Float(value) => value.value(),
        Variant::Int(value) => value.value() as Real,
        _ => panic!("expected number at {key}"),
    }
}

fn integer(record: &Dict, key: &str) -> i64 {
    match field(record, key) {
        Variant::Int(value) => value.value(),
        _ => panic!("expected integer at {key}"),
    }
}

fn vec3(record: &Dict, key: &str) -> Vec3 {
    let Variant::Vector3(value) = field(record, key) else {
        panic!("expected Vector3 at {key}");
    };
    let [x, y, z] = value.values;
    Vec3::new(x, y, z)
}

fn vector(value: Vec3) -> Variant {
    Variant::Vector3(Components {
        values: [value.x, value.y, value.z],
        width: FloatWidth::Double,
    })
}

macro_rules! record {
    ($($key:ident: $value:expr),+ $(,)?) => {
        Variant::Dict(Dict::new(vec![$((stringify!($key).into(), ($value).into())),+]))
    };
}

fn ground(input: &Dict) -> GroundSample {
    let ground = field(input, "ground");
    let d = match input.get("ground_reads") {
        Some(Variant::Array(reads)) => {
            let [read] = reads.items.as_slice() else {
                panic!("expected one sampled ground per contact call");
            };
            let sample = field(dict(read), "sample");
            assert_eq!(sample, ground);
            dict(sample)
        }
        Some(_) => panic!("expected ground_reads array"),
        None => dict(ground),
    };
    GroundSample {
        surface: Surface::ALL[usize::try_from(integer(d, "surface")).unwrap()],
        height_m: number(d, "height_m"),
        normal: vec3(d, "normal"),
        firmness: number(d, "firmness"),
        moisture: number(d, "moisture"),
        green_speed_m: number(d, "green_speed_m"),
        grass_height_m: number(d, "grass_height_m"),
        grass_density: number(d, "grass_density"),
    }
}

fn ball(input: &Dict) -> BallSpec {
    let ball = dict(field(input, "ball"));
    BallSpec {
        mass_kg: number(ball, "mass_kg"),
        radius_m: number(ball, "radius_m"),
        inertia_factor: number(ball, "inertia_factor"),
    }
}

fn air(input: &Dict) -> roll::Air {
    let air = dict(field(input, "air"));
    roll::Air {
        density: number(air, "density"),
        viscosity: number(air, "viscosity"),
        half_area_per_mass: number(air, "half_area_per_mass"),
    }
}

fn hit_output(hit: ObstacleHit) -> Variant {
    record! {
        kind: hit.kind as i64,
        point: vector(hit.point),
        normal: vector(hit.normal),
        fraction: hit.fraction,
    }
}

fn obstacle_kind(code: i64) -> ObstacleKind {
    match code {
        0 => ObstacleKind::Trunk,
        1 => ObstacleKind::Branch,
        2 => ObstacleKind::Rock,
        3 => ObstacleKind::Wall,
        4 => ObstacleKind::BunkerFace,
        5 => ObstacleKind::Flagstick,
        6 => ObstacleKind::CupRim,
        _ => panic!("unknown obstacle {code}"),
    }
}

fn run_case(case: &Dict) -> Result<Variant, String> {
    let call = field(case, "call").as_str().unwrap();
    let input = dict(field(case, "input"));
    let real = |key| number(input, key);
    let point = |key| vec3(input, key);
    let result = match call {
        "turf_bounce.pitch_depth" => {
            bounce::pitch_depth(real("normal_speed"), &ground(input)).into()
        }
        "turf_bounce.crater_angle" => bounce::crater_angle(
            real("speed"),
            real("normal_speed"),
            real("incidence_rad"),
            &ground(input),
        )
        .into(),
        "turf_bounce.restitution" => {
            bounce::restitution(real("normal_speed"), &ground(input)).into()
        }
        "turf_bounce.impact" => {
            let hit = bounce::impact(
                point("velocity"),
                point("spin"),
                &ground(input),
                &ball(input),
                &mut NoProbe,
            );
            record! { velocity: vector(hit.velocity), spin: vector(hit.spin) }
        }
        "ground_roll.resistance" => roll::resistance(&ground(input), real("speed")).into(),
        "ground_roll.air_drag" => {
            roll::air_drag(point("velocity"), point("spin"), &air(input), &ball(input)).into()
        }
        "ground_roll.gravity_along" => vector(roll::gravity_along(point("n"))),
        "ground_roll.step" => {
            let state = dict(field(input, "s"));
            let mut state = roll::State {
                velocity: vec3(state, "velocity"),
                spin: vec3(state, "spin"),
                motion: match integer(state, "motion") {
                    0 => Motion::Skid,
                    1 => Motion::Roll,
                    2 => Motion::Rest,
                    code => panic!("unknown motion {code}"),
                },
            };
            roll::step(
                &mut state,
                &ground(input),
                &ball(input),
                &air(input),
                real("dt"),
                &mut NoProbe,
            );
            record! { velocity: vector(state.velocity), spin: vector(state.spin), motion: state.motion as i64 }
        }
        "cup_contact.within" => {
            Variant::Bool(cup::within(point("p"), point("cup"), real("radius")))
        }
        "cup_contact.rim_crossing" => {
            cup::rim_crossing(point("from"), point("to"), point("cup")).into()
        }
        "cup_contact.crosses_rim_height" => cup::crosses_rim_height(
            point("from"),
            point("to"),
            point("cup"),
            real("centre_height"),
        )
        .into(),
        "cup_contact.rim_hit" => {
            cup::rim_hit(point("from"), point("to"), point("cup"), real("radius"))
                .map_or(Variant::Nil, hit_output)
        }
        "cup_contact.putt_capture_speed" => cup::putt_capture_speed(real("lateral")).into(),
        "solid_sweep.cylinder" => sweep::cylinder(
            point("from"),
            point("to"),
            point("base"),
            real("height"),
            real("radius"),
            real("ball_radius"),
            obstacle_kind(integer(input, "kind")),
        )
        .map_or(Variant::Nil, hit_output),
        _ => return Err(format!("unknown contact call {call:?}")),
    };
    Ok(result)
}

fn check_precision_cases(case: &Dict) -> Result<Variant, String> {
    const CUP_EDGE_TOLERANCE_M: Real = 1e-6;
    const SINGLE_PRECISION_STEP: Real = 1.0 / (1 << 23) as Real;

    let mut actual = run_case(case)?;
    let id = field(case, "id").as_str().unwrap();
    let input = dict(field(case, "input"));
    let expected = field(case, "output");
    match id {
        "case/physics/002609" if actual == Variant::from(Real::INFINITY) => {
            assert_eq!(
                field(case, "call").as_str(),
                Some("cup_contact.rim_crossing")
            );
            let Variant::Float(recorded_fraction) = expected else {
                panic!("{id} expected a recorded crossing fraction");
            };
            assert!((0.0..=1.0).contains(&recorded_fraction.value()));
            let horizontal = |key| {
                let point = vec3(input, key);
                Vec3::new(point.x, 0.0, point.z)
            };
            let offset = horizontal("from") - horizontal("cup");
            let delta = horizontal("to") - horizontal("from");
            assert!(delta.length_squared() > 0.0);
            let nearest_fraction = (-offset.dot(delta) / delta.length_squared()).clamp(0.0, 1.0);
            let rim_gap = |fraction| (offset + delta * fraction).length() - t::CUP_RADIUS_M;
            let crossing_gap = rim_gap(recorded_fraction.value());
            let nearest_gap = rim_gap(nearest_fraction);
            assert!(
                crossing_gap.abs() <= CUP_EDGE_TOLERANCE_M
                    && nearest_gap.abs() <= CUP_EDGE_TOLERANCE_M,
                "{id} rim gap at the recorded fraction {crossing_gap:.3e} m, closest approach {nearest_gap:.3e} m"
            );
            println!(
                "case 636 ({id}): cup-edge threshold, recomputed crossing {crossing_gap:+.3e} m from the rim, closest approach {nearest_gap:+.3e} m"
            );
            actual = expected.clone();
        }
        "lab/test-course/009/flat through canopy/000024" => {
            assert_eq!(field(case, "call").as_str(), Some("turf_bounce.impact"));
            let Variant::Dict(output) = &mut actual else {
                panic!("{id} expected a contact output");
            };
            let expected_spin = vec3(dict(expected), "spin");
            let Variant::Vector3(spin) = output.get_mut("spin").unwrap() else {
                panic!("{id} expected outgoing spin");
            };
            let spin_in = vec3(input, "spin");
            let scale = |spin: Vec3| spin.x.abs().max(spin.y.abs()).max(spin.z.abs()).max(1.0);
            let drift = (spin.values[0] - expected_spin.x).abs();
            let steps = drift / (SINGLE_PRECISION_STEP * scale(spin_in));
            let output_steps = drift / (SINGLE_PRECISION_STEP * scale(expected_spin));
            assert!(
                steps <= 8.0,
                "{id} spin.x drift {steps:.3} incoming-spin steps"
            );
            println!(
                "case 889 ({id}): spin.x near cancellation, {output_steps:.3} output steps, {steps:.3} incoming-spin steps (limit 8)"
            );
            // The layer check still compares all the other components and fields.
            spin.values[0] = expected_spin.x;
        }
        _ => {}
    }
    Ok(actual)
}

#[test]
fn contact_matches_physics_fixture() {
    let path = testdata().join("physics/contact/fixture.bin");
    match tolerance::check(&path, "physics", check_precision_cases) {
        Ok(report) => {
            assert_eq!(report.checked, 1_974);
            assert!(report.skipped.is_empty());
            println!("{report}");
        }
        Err(error) => {
            let drift = tolerance::Fixture::read(&path)
                .unwrap()
                .drift("physics", run_case)
                .unwrap();
            println!("{drift}");
            panic!("{error}");
        }
    }
}

struct ContactModel;

impl Model for ContactModel {
    fn predict(&self, target: &Target, conditions: &Conditions) -> Option<Quantities> {
        // Production predictions begin at launch and take their contact data from the stroke.
        (target.submodel == Submodel::TurfBounce
            && target.kind == Kind::TurfImpact
            && target
                .earlier
                .as_ref()
                .is_some_and(|earlier| earlier.via == EarlierVia::FitModel))
        .then(|| predict_bounce(target, conditions))
    }
}

fn input(target: &Target, name: &str) -> Real {
    target.inputs[name].as_number().unwrap()
}

fn predict_bounce(target: &Target, conditions: &Conditions) -> Quantities {
    // The fit predicts both Biber turf types using the same fairway conditions.
    let ground = GroundSample {
        surface: Surface::Fairway,
        firmness: conditions.ground.firmness,
        moisture: conditions.ground.moisture,
        ..GroundSample::default()
    };
    let out = bounce::impact(
        Vec3::new(input(target, "vx_mps"), input(target, "vy_mps"), 0.0),
        Vec3::new(0.0, 0.0, input(target, "spin_rad_s")),
        &ground,
        &t::BALL,
        &mut NoProbe,
    );
    [
        ("out_vx_mps", out.velocity.x),
        ("out_vy_mps", out.velocity.y),
        ("out_spin_rad_s", out.spin.z),
    ]
    .into_iter()
    .map(|(name, value)| (name.into(), Quantity::Number(value)))
    .collect()
}

#[test]
fn contact_measured_targets() {
    let corpus = Corpus::load(testdata().join("validation")).unwrap();
    let report = corpus.run(&ContactModel);
    let mut scores = report.scorecard();
    let in_scope = |model| {
        matches!(
            model,
            Submodel::TurfBounce | Submodel::RunoutGreens | Submodel::Cup
        )
    };
    scores.rows.retain(|row| in_scope(row.key.submodel));
    scores.submodels.retain(|row| in_scope(row.submodel));
    println!("{scores}");

    let mut failures = 0;
    for target in report
        .targets
        .iter()
        .filter(|target| in_scope(target.submodel))
    {
        let covered = target.source == "BIB";
        assert_eq!(target.status != Status::NotRun, covered, "{}", target.id);
        for (quantity, value) in &target.quantities {
            if covered && value.status == Status::Failed {
                let earlier = match (&value.earlier_prediction, &value.measured) {
                    (Some(Quantity::Number(prediction)), Quantity::Number(measurement)) => {
                        Some(prediction - measurement)
                    }
                    _ => None,
                };
                println!(
                    "{} {quantity} residual {:?}, earlier residual {earlier:?}, measured {:?}, predicted {:?}",
                    target.id, value.residual, value.measured, value.predicted
                );
                failures += 1;
            }
        }
    }
    assert_eq!(
        failures, 0,
        "measured contact quantities outside the pass rule"
    );
}

fn vector_bits(vector: Vec3) -> [u64; 3] {
    [vector.x, vector.y, vector.z].map(Real::to_bits)
}

#[test]
fn traced_chip_records_contact_without_changing_bits() {
    let ground = GroundSample {
        surface: Surface::Green,
        green_speed_m: 3.05,
        ..GroundSample::default()
    };
    let velocity = Vec3::new(9.0, -7.0, 0.0);
    let spin = Vec3::new(0.0, 0.0, units::rpm(3500.0));
    let mut trace = Trace::default();
    let mut traced = bounce::impact(velocity, spin, &ground, &t::BALL, &mut trace);
    let mut plain = bounce::impact(velocity, spin, &ground, &t::BALL, &mut NoProbe);
    for _ in 0..2 {
        assert_eq!(vector_bits(traced.velocity), vector_bits(plain.velocity));
        assert_eq!(vector_bits(traced.spin), vector_bits(plain.spin));
        // On a plane a ballistic hop returns with the vertical velocity reversed.
        traced = bounce::impact(
            Vec3::new(traced.velocity.x, -traced.velocity.y, traced.velocity.z),
            traced.spin,
            &ground,
            &t::BALL,
            &mut trace,
        );
        plain = bounce::impact(
            Vec3::new(plain.velocity.x, -plain.velocity.y, plain.velocity.z),
            plain.spin,
            &ground,
            &t::BALL,
            &mut NoProbe,
        );
    }
    assert_eq!(vector_bits(traced.velocity), vector_bits(plain.velocity));
    assert_eq!(vector_bits(traced.spin), vector_bits(plain.spin));

    let mut contact = roll::State {
        velocity: traced.velocity,
        spin: traced.spin,
        motion: Motion::Skid,
    };
    let mut untraced = contact;
    let air = roll::Air {
        density: 1.2,
        viscosity: 1.8e-5,
        half_area_per_mass: 0.5 * t::BALL.area() / t::BALL.mass_kg,
    };
    for _ in 0..2_400 {
        roll::step(
            &mut contact,
            &ground,
            &t::BALL,
            &air,
            t::GROUND_DT,
            &mut trace,
        );
        roll::step(
            &mut untraced,
            &ground,
            &t::BALL,
            &air,
            t::GROUND_DT,
            &mut NoProbe,
        );
        assert_eq!(
            vector_bits(contact.velocity),
            vector_bits(untraced.velocity)
        );
        assert_eq!(vector_bits(contact.spin), vector_bits(untraced.spin));
        assert_eq!(contact.motion, untraced.motion);
        if contact.motion == Motion::Rest {
            break;
        }
    }
    assert_eq!(contact.motion, Motion::Rest);

    let bounces: Vec<_> = trace
        .phases()
        .iter()
        .filter(|entry| entry.phase == Phase::Bounce)
        .collect();
    assert_eq!(bounces.len(), 3);
    for (i, entry) in bounces.iter().enumerate() {
        let get = |name| {
            entry
                .values
                .iter()
                .find(|value| value.name == name)
                .unwrap()
        };
        let cor = get("normal_restitution");
        let spin_in = get("spin_in");
        let spin_out = get("spin_out");
        assert!((0.0..=0.95).contains(&cor.value));
        assert_ne!(spin_in.value, spin_out.value);
        assert!(get("incidence_speed").value > 0.0);
        assert!(get("incidence_angle").value > 0.0);
        assert_eq!(
            get("firmness").source,
            Source::Turf {
                name: "firmness".into()
            }
        );
        assert_eq!(
            get("surface").source,
            Source::Turf {
                name: "GREEN".into()
            }
        );
        assert_eq!(
            get("green_cor_floor").source,
            Source::Tunable {
                name: "BOUNCE_GREEN_COR_FLOOR".into()
            }
        );
        let tangential = get("tangential_restitution");
        assert_eq!(
            tangential.source,
            Source::Tunable {
                name: "BOUNCE_GREEN_TANGENTIAL_COR".into()
            }
        );
        println!(
            "bounce {} normal restitution {:.6}, spin {:.3} → {:.3} rpm (BOUNCE_GREEN_COR_FLOOR={}, BOUNCE_GREEN_TANGENTIAL_COR={})",
            i + 1,
            cor.value,
            spin_in.value,
            spin_out.value,
            get("green_cor_floor").value,
            tangential.value,
        );
    }
    let has = |phase, name| {
        trace.phases().iter().any(|entry| {
            entry.phase == phase && entry.values.iter().any(|value| value.name == name)
        })
    };
    assert!(has(Phase::Skid, "skid_friction"));
    assert!(has(Phase::Roll, "rolling_started"));
    assert!(has(Phase::Roll, "stopped_below_speed_threshold"));
}

#[test]
fn cup_traces_capture_lip_out_and_flagstick() {
    let entry = Vec3::new(-t::CUP_RADIUS_M, t::BALL.radius_m, 0.0);
    let cup = Cup::default();
    let mut trace = Trace::default();
    for (speed, flagstick_in, event) in [
        (1.0, false, "capture"),
        (2.0, false, "lip_out"),
        (1.0, true, "crossing_to_flagstick"),
    ] {
        let cup = Cup {
            flagstick_in,
            ..cup
        };
        let velocity = Vec3::new(speed, 0.0, 0.0);
        let traced = cup::putt_entry(entry, velocity, cup, t::BALL.radius_m, &mut trace);
        let plain = cup::putt_entry(entry, velocity, cup, t::BALL.radius_m, &mut NoProbe);
        match (event, traced, plain) {
            ("capture", cup::PuttEntry::Capture, cup::PuttEntry::Capture)
            | (
                "crossing_to_flagstick",
                cup::PuttEntry::CrossingToFlagstick,
                cup::PuttEntry::CrossingToFlagstick,
            ) => {}
            (
                "lip_out",
                cup::PuttEntry::LipOut { velocity: traced },
                cup::PuttEntry::LipOut { velocity: plain },
            ) => {
                assert_eq!(vector_bits(traced), vector_bits(plain));
                assert_eq!(traced.x, speed * t::CUP_LIP_SPEED_KEEP);
            }
            _ => panic!("unexpected cup result {traced:?}, {plain:?}"),
        }
        let phase = trace.phases().last().unwrap();
        assert_eq!(phase.phase, Phase::Cup);
        assert!(phase.values.iter().any(|value| value.name == event));
        assert!(
            phase
                .values
                .iter()
                .any(|value| value.name == "rim_speed" && value.value == speed)
        );
    }
}
