use golf_sim_base::{
    Real, Vec3,
    variant::{Components, Dict, FloatWidth, Variant},
};
use golf_sim_physics::{
    air::{self, Atmosphere, FlightAir},
    flight::{self},
    samplers::AirSample,
    values::BallSpec,
};

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

pub(in crate::corpus) fn replay(case: &Dict) -> Result<Variant, String> {
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
