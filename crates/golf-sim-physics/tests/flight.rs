use std::path::{Path, PathBuf};

use golf_sim_base::{
    Real, Vec2, Vec3, fm,
    num::{deg_to_rad, rad_to_deg},
    trace::{NoProbe, Phase, Probe, Source, Trace},
    variant::{Components, Dict, FloatWidth, Variant},
};
use golf_sim_physics::{
    air::{self, Atmosphere, FlightAir},
    flight::{self, FlightPoint, FlightTrace},
    samplers::AirSample,
    tunables as t,
    values::{BallSpec, Launch, LaunchReadout, units},
};
use golf_sim_testkit::tolerance;
use golf_sim_validate::{
    Conditions, Corpus, Kind, Model, Quantities, Quantity, Status, Submodel, Target,
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

fn vec3(record: &Dict, key: &str) -> Vec3 {
    let Variant::Vector3(v) = field(record, key) else {
        panic!("expected Vector3 at {key}");
    };
    let [x, y, z] = v.values;
    Vec3::new(x, y, z)
}

fn vector(v: Vec3) -> Variant {
    Variant::Vector3(Components {
        values: [v.x, v.y, v.z],
        width: FloatWidth::Double,
    })
}

macro_rules! record {
    ($($key:ident: $value:expr),+ $(,)?) => {
        Variant::Dict(Dict::new(vec![$((stringify!($key).into(), ($value).into())),+]))
    };
}

fn air_sample(value: &Variant) -> AirSample {
    let d = dict(value);
    AirSample {
        temperature_c: number(d, "temperature_c"),
        pressure_pa: number(d, "pressure_pa"),
        relative_humidity: number(d, "relative_humidity"),
        mean_wind_mps: vec3(d, "mean_wind_mps"),
        gust_mps: vec3(d, "gust_mps"),
        wind_reference_height_m: number(d, "wind_reference_height_m"),
        roughness_length_m: number(d, "roughness_length_m"),
    }
}

fn flight_air(d: &Dict) -> FlightAir {
    FlightAir {
        density_kg_m3: number(d, "density"),
        viscosity_pa_s: number(d, "viscosity"),
        wind_mps: vec3(d, "wind"),
        canopy_density: number(d, "canopy"),
    }
}

fn replay_air_update(input: &Dict) -> Variant {
    let before = field(input, "self");
    let before_dict = dict(before);
    let previous = AirSample {
        temperature_c: number(before_dict, "temperature"),
        pressure_pa: number(before_dict, "pressure"),
        relative_humidity: number(before_dict, "humidity"),
        ..AirSample::default()
    };
    let sample = air_sample(field(input, "sample"));
    let mut atmosphere = Atmosphere::default();
    // Seed the cache from the recorded inputs when it had already been initialized.
    if previous.temperature_c.is_finite() {
        assert!(atmosphere.sample(&previous, 0.0, 0.0).is_some());
    }
    let Some(air) = atmosphere.sample(&sample, number(input, "height"), number(input, "foliage"))
    else {
        return record! { valid: Variant::Bool(false), air: before.clone() };
    };
    record! {
        valid: Variant::Bool(true),
        air: record! {
            density: air.density_kg_m3,
            viscosity: air.viscosity_pa_s,
            wind: vector(air.wind_mps),
            canopy: air.canopy_density,
            half_area_per_mass: number(before_dict, "half_area_per_mass"),
            temperature: sample.temperature_c,
            pressure: sample.pressure_pa,
            humidity: sample.relative_humidity,
        },
    }
}

fn replay(case: &Dict) -> Result<Variant, String> {
    let call = field(case, "call").as_str().unwrap();
    let input = dict(field(case, "input"));
    let real = |key| number(input, key);
    let sample = || air_sample(field(input, "air"));
    let result = match call {
        "atmosphere.valid" => {
            Variant::Bool(!matches!(field(input, "air"), Variant::Nil) && air::valid(&sample()))
        }
        "atmosphere.saturation_vapor_pressure_pa" => {
            air::saturation_vapor_pressure_pa(real("temperature_c")).into()
        }
        "atmosphere.density" => air::density(&sample()).into(),
        "atmosphere.viscosity" => air::viscosity(&sample()).into(),
        "atmosphere.wind_at" => vector(air::wind_at(&sample(), real("height_m"))),
        "atmosphere.isa_pressure_pa" => air::isa_pressure_pa(real("altitude_m")).into(),
        "aero.drag" => flight::drag(real("re"), real("s")).into(),
        "aero.lift" => flight::lift(real("s")).into(),
        "aero.coefficients" => {
            let (cd, cl) = flight::coefficients(real("re"), real("s"));
            Variant::Vector2(Components {
                values: [cd, cl],
                width: FloatWidth::Double,
            })
        }
        "ball_flight.Air.update" => replay_air_update(input),
        "ball_flight.acceleration" | "ball_flight.step" | "ball_flight.decay_spin" => {
            let recorded_air = dict(field(input, "air"));
            let air = flight_air(recorded_air);
            let recorded_ball = dict(field(input, "ball"));
            let ball = BallSpec {
                mass_kg: number(recorded_ball, "mass_kg"),
                radius_m: number(recorded_ball, "radius_m"),
                inertia_factor: number(recorded_ball, "inertia_factor"),
            };
            let spin = vec3(input, "spin");
            match call {
                "ball_flight.acceleration" => vector(flight::acceleration(
                    vec3(input, "velocity"),
                    spin,
                    &air,
                    &ball,
                )),
                "ball_flight.step" => {
                    let step = flight::step(vec3(input, "velocity"), spin, &air, &ball, real("dt"));
                    record! { travel: vector(step.travel), velocity: vector(step.velocity) }
                }
                _ => vector(flight::decay_spin(
                    spin,
                    vec3(input, "before"),
                    vec3(input, "after"),
                    &air,
                    &ball,
                    real("dt"),
                )),
            }
        }
        _ => return Err(format!("unknown flight call {call:?}")),
    };
    Ok(result)
}

#[test]
fn matches_flight_fixture() {
    let path = testdata().join("physics/flight/fixture.bin");
    let report = tolerance::check(&path, "physics", replay).unwrap_or_else(|error| {
        if let Ok(fixture) = tolerance::Fixture::read(&path)
            && let Ok(drift) = fixture.drift("physics", replay)
        {
            eprint!("{drift}");
        }
        panic!("{error}");
    });
    assert_eq!(report.checked, 3103);
    assert!(report.skipped.is_empty());
    assert!(report.discrete.is_empty());
    print!("{report}");
}

fn fly(launch: LaunchReadout, sample: &AirSample, probe: &mut impl Probe) -> (FlightPoint, Real) {
    let ball = t::standard_ball();
    let launch = Launch::from_readout(&ball, Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), launch);
    let mut from = FlightPoint {
        time_s: 0.0,
        position: launch.position,
        velocity: launch.velocity,
        spin: launch.spin,
    };
    let mut trace = FlightTrace::begin(probe, from);
    let mut atmosphere = Atmosphere::default();
    let mut apex = None;
    while from.time_s < t::MAX_SHOT_TIME_S {
        let air = atmosphere.sample(sample, from.position.y, 0.0).unwrap();
        let step = flight::step(from.velocity, from.spin, &air, &ball, t::FLIGHT_DT);
        let next_position = from.position + step.travel;
        let clearance = next_position.y - ball.radius_m;
        let landed = clearance <= 0.0;
        let fraction = if landed {
            let previous_clearance = from.position.y - ball.radius_m;
            if previous_clearance > 0.0 {
                previous_clearance / (previous_clearance - clearance)
            } else {
                0.0
            }
        } else {
            1.0
        };
        let dt = t::FLIGHT_DT * fraction;
        let velocity = if fraction < 1.0 {
            from.velocity.lerp(step.velocity, fraction)
        } else {
            step.velocity
        };
        let mut to = FlightPoint {
            time_s: from.time_s + dt,
            position: from.position + step.travel * fraction,
            velocity,
            spin: flight::decay_spin(from.spin, from.velocity, velocity, &air, &ball, dt),
        };
        apex = apex.or_else(|| from.apex_between(to));
        trace.record_step(from, to);
        if landed {
            to.position.y = ball.radius_m;
            trace.land(to);
            return (to, apex.unwrap().position.y - launch.position.y);
        }
        from = to;
    }
    panic!("flight never reached the ground");
}

fn launch_readout(target: &Target) -> LaunchReadout {
    let input = |name| target.inputs[name].as_number().unwrap();
    LaunchReadout {
        ball_speed_mps: units::mph(input("ball_mph")),
        launch_angle_rad: deg_to_rad(input("launch_deg")),
        spin_rate_rad_s: units::rpm(input("spin_rpm")),
        ..LaunchReadout::default()
    }
}

fn carry(landing: FlightPoint) -> Real {
    units::to_yards(Vec2::new(landing.position.x, landing.position.z).length())
}

struct FlightModel<'a> {
    gain_baseline: &'a Target,
}

impl Model for FlightModel<'_> {
    fn predict(&self, target: &Target, conditions: &Conditions) -> Option<Quantities> {
        if target.submodel != Submodel::Flight {
            return None;
        }
        if target.kind == Kind::Aero {
            let input = |name| target.inputs[name].as_number().unwrap();
            let (cd, cl) = flight::coefficients(input("reynolds"), input("spin_ratio"));
            return Some(
                [
                    ("cd".into(), Quantity::Number(cd)),
                    ("cl".into(), Quantity::Number(cl)),
                ]
                .into(),
            );
        }

        assert_eq!(target.kind, Kind::Launch);
        assert_eq!(conditions.ball, "standard");
        let mut sample = AirSample {
            temperature_c: conditions.air_temp_c,
            pressure_pa: conditions.pressure_pa,
            relative_humidity: conditions.humidity,
            ..AirSample::default()
        };
        let wind_mph = target
            .inputs
            .get("wind_mph")
            .map_or(conditions.wind_mph, |wind| wind.as_number().unwrap());
        if wind_mph != 0.0 {
            // The wind targets approximate height-independent wind with tiny roughness.
            sample.mean_wind_mps.z = units::mph(wind_mph);
            sample.roughness_length_m = 1e-6;
            sample.wind_reference_height_m = 15.0;
        }

        let launch = launch_readout(target);
        let (landing, apex) = fly(launch, &sample, &mut NoProbe);
        let carry_yd = carry(landing);
        let horizontal_speed = Vec2::new(landing.velocity.x, landing.velocity.z).length();
        let descent_deg = rad_to_deg(fm::atan2(-landing.velocity.y, horizontal_speed));
        let mut quantities = Quantities::from([
            ("carry_yd".into(), Quantity::Number(carry_yd)),
            ("apex_yd".into(), Quantity::Number(units::to_yards(apex))),
            ("descent_deg".into(), Quantity::Number(descent_deg)),
        ]);
        if target.measured.contains_key("carry_gain_yd") {
            let baseline = fly(
                launch_readout(self.gain_baseline),
                &sample,
                &mut NoProbe,
            )
            .0;
            quantities.insert(
                "carry_gain_yd".into(),
                Quantity::Number(carry_yd - carry(baseline)),
            );
        }
        if target.measured.contains_key("carry_change_yd") {
            sample.mean_wind_mps = Vec3::ZERO;
            let baseline = fly(launch, &sample, &mut NoProbe).0;
            quantities.insert(
                "carry_change_yd".into(),
                Quantity::Number(carry_yd - carry(baseline)),
            );
        }
        Some(quantities)
    }
}

#[test]
fn meets_measured_flight_targets() {
    let corpus = Corpus::load(testdata().join("validation")).unwrap();
    let gain_baseline = corpus
        .files()
        .iter()
        .flat_map(|file| &file.targets)
        .find(|target| target.id == "X4.attack-1.5.launch")
        .expect("carry-gain baseline launch");
    let mut report = corpus.run(&FlightModel { gain_baseline });
    report
        .targets
        .retain(|target| target.submodel == Submodel::Flight);
    assert!(!report.targets.is_empty());
    let mut scorecard = report.scorecard();
    scorecard
        .rows
        .retain(|row| row.key.submodel == Submodel::Flight);
    scorecard
        .submodels
        .retain(|row| row.submodel == Submodel::Flight);
    print!("{scorecard}");

    let mut failures = Vec::new();
    for target in report.targets {
        for (name, quantity) in target.quantities {
            if quantity.predicted.is_none()
                || ((quantity.earlier_prediction.is_some() || quantity.source_tolerance.is_some())
                    && quantity.status != Status::Passed)
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

fn point_bits(point: FlightPoint) -> [u64; 10] {
    [
        point.time_s,
        point.position.x,
        point.position.y,
        point.position.z,
        point.velocity.x,
        point.velocity.y,
        point.velocity.z,
        point.spin.x,
        point.spin.y,
        point.spin.z,
    ]
    .map(Real::to_bits)
}

#[test]
fn traced_flight_records_phases_without_changing_the_result() {
    let launch = LaunchReadout {
        ball_speed_mps: units::mph(171.0),
        launch_angle_rad: deg_to_rad(10.4),
        spin_rate_rad_s: units::rpm(2545.0),
        ..LaunchReadout::default()
    };
    let air = AirSample::default();
    let mut trace = Trace::default();
    let (traced, traced_apex) = fly(launch, &air, &mut trace);
    let (untraced, untraced_apex) = fly(launch, &air, &mut NoProbe);
    assert_eq!(point_bits(traced), point_bits(untraced));
    assert_eq!(traced_apex.to_bits(), untraced_apex.to_bits());

    let [launch, flight] = trace.phases() else {
        panic!("expected launch and flight");
    };
    assert_eq!((launch.phase, flight.phase), (Phase::Launch, Phase::Flight));
    let values = |name| {
        flight
            .values
            .iter()
            .filter(move |v| v.name == name)
            .map(|v| v.value)
    };
    assert_eq!(launch.values[0].name, "launch_time_s");
    assert_eq!(launch.values[0].value, 0.0);
    assert_eq!(values("apex_time_s").count(), 1);
    assert_eq!(
        values("apex_position_y").next().unwrap(),
        traced_apex + t::standard_ball().radius_m
    );
    assert_eq!(
        values("landing_time_s").collect::<Vec<_>>(),
        [traced.time_s]
    );
    assert_eq!(
        values("landing_position_y").next().unwrap(),
        traced.position.y
    );
    let sample_times: Vec<_> = values("sample_time_s").collect();
    assert_eq!(
        sample_times.len(),
        fm::floor(traced.time_s / t::SAMPLE_INTERVAL_S) as usize
    );
    for (index, sample) in (1_u32..).zip(&sample_times) {
        assert_eq!(*sample, Real::from(index) * t::SAMPLE_INTERVAL_S);
    }
    for (label, expected) in [
        (
            "drag",
            &[
                "CD_BASE",
                "CD_SPIN",
                "CD_SPIN2",
                "CD_SPIN_PEAK",
                "CD_CRISIS_RE",
                "CD_CRISIS_WIDTH",
                "CD_CRISIS_RISE",
            ][..],
        ),
        ("lift", &["CL_SCALE", "CL_EXPONENT"][..]),
    ] {
        let names: Vec<_> = flight
            .values
            .iter()
            .filter(|entry| entry.name == label)
            .map(|entry| {
                let Source::Tunable { name } = &entry.source else {
                    panic!("{label} should name a tunable");
                };
                name.as_str()
            })
            .collect();
        assert_eq!(names, expected);
    }
    println!(
        "launch, {} samples, apex and landing recorded; 7 drag and 2 lift tunables named; identical result bits",
        sample_times.len()
    );
}
