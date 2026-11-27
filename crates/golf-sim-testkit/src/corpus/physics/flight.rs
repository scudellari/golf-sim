use golf_sim_base::{
    Real, Vec2, Vec3, fm,
    num::smoothstep,
    variant::{Components, Dict, FloatWidth, Variant},
};
use golf_sim_physics::{
    air::{self, Atmosphere, FlightAir},
    flight::{self},
    samplers::{AirSample, AirSampler, GroundSampler},
    tunables as t,
    values::{BallSpec, Launch},
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

#[derive(Clone, Copy)]
struct FlightPoint {
    at: Vec3,
    height_error: Real,
}

fn descending_crossing(
    points: &[FlightPoint],
    origin: Vec3,
    level: Real,
    offset: Real,
) -> Option<Real> {
    let height = |point: FlightPoint| point.at.y + offset * point.height_error;
    points.windows(2).find_map(|pair| {
        let (a, b) = (height(pair[0]), height(pair[1]));
        if a <= level || b > level {
            return None;
        }
        let to_crossing = pair[0].at.lerp(pair[1].at, (a - level) / (a - b)) - origin;
        Some(Vec2::new(to_crossing.x, to_crossing.z).length())
    })
}

fn verifies_carry(points: &[FlightPoint], origin: Vec3, level: Real, carry: Real) -> bool {
    descending_crossing(points, origin, level, 0.0)
        .is_some_and(|exact| (carry - exact).abs() <= 1e-10 * exact.max(1.0))
}

pub(in crate::corpus) fn carry_rounding_bound(
    launch: &Launch,
    ball: &BallSpec,
    time: Real,
    air: &dyn AirSampler,
    ground: &dyn GroundSampler,
    level: Real,
    carry: Real,
) -> Real {
    let mut at = launch.position;
    let mut velocity = launch.velocity;
    let mut spin = launch.spin;
    let first_ground = ground.sample_ground(at.x, at.z);
    let normal_speed = velocity.dot(first_ground.normal);
    let clearance = (at.y - first_ground.height_m) * first_ground.normal.y - ball.radius_m;
    if clearance <= t::CONTACT_SLOP_M && normal_speed < -t::CONTACT_SLOP_M {
        return Real::NAN;
    }
    if clearance <= t::SUPPORT_BLEND_HEIGHT_M {
        let release = fm::sqrt(2.0 * t::GRAVITY * first_ground.normal.y * t::MIN_HOP_HEIGHT_M);
        let support = 1.0
            - smoothstep(
                t::SUPPORTED_LIE_HEIGHT_M,
                t::SUPPORT_BLEND_HEIGHT_M,
                clearance,
            );
        let absorbed = support * (1.0 - smoothstep(release, release * 3.0, normal_speed));
        velocity -= first_ground.normal * (normal_speed.min(release) * absorbed);
        if velocity.dot(first_ground.normal) <= t::CONTACT_SLOP_M {
            return Real::NAN;
        }
    }

    let level = level + ball.radius_m;
    let mut atmosphere = Atmosphere::default();
    let mut elapsed = 0.0;
    let step = Real::from(f32::EPSILON);
    let mut height_error = 0.0;
    let mut travel_error = 0.0;
    let mut points = vec![FlightPoint { at, height_error }];
    while elapsed < t::MAX_SHOT_TIME_S {
        let sample = air.sample_air(time + elapsed, at);
        let surface = ground.sample_ground(at.x, at.z);
        let Some(air) = atmosphere.sample(&sample, at.y - surface.height_m, 0.0) else {
            return Real::NAN;
        };
        let next = flight::step(velocity, spin, &air, ball, t::FLIGHT_DT);
        spin = flight::decay_spin(spin, velocity, next.velocity, &air, ball, t::FLIGHT_DT);
        at += next.travel;
        velocity = next.velocity;
        elapsed += t::FLIGHT_DT;
        // Each reference vector update rounds the accumulated height and vertical
        // velocity. Sum their contributions up to the two crossing brackets.
        travel_error += step * next.travel.y.abs();
        height_error += step * at.y.abs() + travel_error;
        points.push(FlightPoint { at, height_error });
        if velocity.y < 0.0 && at.y + height_error <= level {
            break;
        }
    }
    if !verifies_carry(&points, launch.position, level, carry) {
        return Real::NAN;
    }
    let range = [-1.0, 1.0].map(|sign| descending_crossing(&points, launch.position, level, sign));
    match range {
        [Some(low), Some(high)] => (low - carry).abs().max((high - carry).abs()),
        _ => Real::NAN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golf_sim_physics::{
        integrator::carry_to_height,
        plane::{ConstantAir, PlaneGround},
        samplers::GroundSample,
        tunables::standard_ball,
        values::Launch,
    };

    #[test]
    fn an_incorrect_carry_cannot_use_the_height_rounding_envelope() {
        let points = [
            FlightPoint {
                at: Vec3::new(2.0, 0.0, 0.0),
                height_error: 0.01,
            },
            FlightPoint {
                at: Vec3::new(4.0, 2.0, 0.0),
                height_error: 0.01,
            },
            FlightPoint {
                at: Vec3::new(8.0, 0.0, 0.0),
                height_error: 0.01,
            },
        ];
        assert!(verifies_carry(&points, Vec3::ZERO, 1.0, 6.0));
        assert!(!verifies_carry(&points, Vec3::ZERO, 1.0, 3.0));
        assert!(!verifies_carry(&points, Vec3::ZERO, 1.0, 6.01));
    }

    #[test]
    fn carry_rounding_tracks_the_height_of_the_reference_vectors() {
        let ball = standard_ball();
        let air = ConstantAir::new(AirSample::default());
        let bound = |level| {
            let launch = Launch {
                position: Vec3::new(0.0, level + 0.1, 0.0),
                velocity: Vec3::new(25.0, 13.0, 0.0),
                ..Launch::default()
            };
            let ground = PlaneGround::new(GroundSample::default(), Vec3::new(0.0, level, 0.0));
            let carry = carry_to_height(&launch, &ball, 0.0, &air, &ground, level);
            carry_rounding_bound(&launch, &ball, 0.0, &air, &ground, level, carry)
        };
        assert!(bound(0.0) > 0.0);
        assert!(bound(512.0) > bound(0.0));
    }
}
