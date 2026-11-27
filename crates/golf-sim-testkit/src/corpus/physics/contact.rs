use golf_sim_base::{
    Real, Surface, Vec2, Vec3, fm,
    trace::NoProbe,
    variant::{Components, Dict, FloatWidth, Variant},
};
use golf_sim_physics::{
    bounce, cup,
    roll::{self, Motion},
    samplers::GroundSample,
    sweep, tunables as t,
    values::{BallSpec, ObstacleHit, ObstacleKind},
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

const SINGLE_PRECISION_STEP: Real = f32::EPSILON as Real;

fn exact_rim_crossing(from: Vec3, to: Vec3, center: Vec3) -> Real {
    let offset = Vec2::new(from.x - center.x, from.z - center.z);
    let delta = Vec2::new(to.x - from.x, to.z - from.z);
    let length = delta.length();
    let radius = t::CUP_RADIUS_M;
    if length == 0.0 {
        return Real::INFINITY;
    }
    if offset.length_squared() < radius * radius {
        return 0.0;
    }
    let direction = delta / length;
    let perpendicular = offset.cross(direction).abs();
    if perpendicular > radius {
        return Real::INFINITY;
    }
    let half_chord = fm::sqrt((radius - perpendicular) * (radius + perpendicular));
    let fraction = (-offset.dot(direction) - half_chord) / length;
    if (0.0..=1.0).contains(&fraction) {
        fraction
    } else {
        Real::INFINITY
    }
}

fn matches_rim_crossing(from: Vec3, to: Vec3, center: Vec3, computed: Real) -> bool {
    let exact = exact_rim_crossing(from, to, center);
    computed == exact || (computed - exact).abs() <= 1e-11
}

fn grazing_rim(from: Vec3, to: Vec3, cup: Vec3, fraction: Real) -> Option<(Real, Real, Real)> {
    let plan = |point: Vec3| Vec3::new(point.x, 0.0, point.z);
    let offset = plan(from) - plan(cup);
    let delta = plan(to) - plan(from);
    let length_squared = delta.length_squared();
    if !(0.0..=1.0).contains(&fraction) || length_squared == 0.0 {
        return None;
    }
    let nearest = -offset.dot(delta) / length_squared;
    if !(0.0..=1.0).contains(&nearest) {
        return None;
    }
    let gap = |t| (offset + delta * t).length() - t::CUP_RADIUS_M;
    // The quadratic starts with local vectors, rounded after translation. Its
    // dot products can move a near-zero discriminant to either side of zero.
    let bound =
        8.0 * SINGLE_PRECISION_STEP * offset.length().max(delta.length()).max(t::CUP_RADIUS_M);
    let nearest_gap = gap(nearest);
    let crossing_gap = gap(fraction);
    (nearest_gap.abs() <= bound && crossing_gap.abs() <= bound).then_some((
        nearest_gap,
        crossing_gap,
        bound,
    ))
}

pub(in crate::corpus) fn run(case: &Dict, drift_report: bool) -> Result<Variant, String> {
    let mut actual = run_case(case)?;
    let id = field(case, "id").as_str().unwrap();
    let input = dict(field(case, "input"));
    let expected = field(case, "output");
    match field(case, "call").as_str().unwrap() {
        "cup_contact.rim_crossing" => {
            let (Variant::Float(recorded), Variant::Float(computed)) = (expected, &actual) else {
                return Err("expected rim crossing fractions".into());
            };
            let recorded = recorded.value();
            let computed = computed.value();
            let [from, to, center] = ["from", "to", "cup"].map(|key| vec3(input, key));
            if !matches_rim_crossing(from, to, center, computed) {
                return Err("core rim crossing differs from the first planar root".into());
            }
            let finite = match (recorded, computed) {
                (recorded, computed) if computed == Real::INFINITY => recorded,
                (recorded, computed) if recorded == Real::INFINITY => computed,
                _ => return Ok(actual),
            };
            if let Some((nearest, crossing, bound)) = grazing_rim(from, to, center, finite) {
                println!(
                    "case {id}: cup-edge threshold, nearest gap {nearest:+.3e} m, crossing gap {crossing:+.3e} m (bound {bound:.3e} m)"
                );
                if !drift_report {
                    actual = expected.clone();
                }
            }
        }
        "turf_bounce.impact" => {
            let Variant::Dict(output) = &mut actual else {
                return Err("expected turf contact output".into());
            };
            let Variant::Vector3(spin) = output.get_mut("spin").unwrap() else {
                return Err("expected outgoing spin".into());
            };
            let incoming = vec3(input, "spin");
            let recorded = vec3(dict(expected), "spin");
            let outgoing = Vec3::new(spin.values[0], spin.values[1], spin.values[2]);
            let scale = |v: Vec3| v.x.abs().max(v.y.abs()).max(v.z.abs()).max(1.0);
            let operand = scale(incoming).max(scale(outgoing - incoming));
            for ((actual, expected), axis) in spin
                .values
                .iter_mut()
                .zip([recorded.x, recorded.y, recorded.z])
                .zip(["x", "y", "z"])
            {
                let drift = (*actual - expected).abs();
                let normal = 1e-6_f64
                    .max(1e-5 * actual.abs().max(expected.abs()))
                    .max(8.0 * SINGLE_PRECISION_STEP * scale(recorded));
                let bound = 8.0 * SINGLE_PRECISION_STEP * operand;
                if drift > normal && drift <= bound {
                    println!(
                        "case {id}: spin.{axis} drift {drift:.3e} rad/s after adding the impulse (bound {bound:.3e} rad/s)"
                    );
                    if !drift_report {
                        *actual = expected;
                    }
                }
            }
        }
        _ => {}
    }
    Ok(actual)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_near_tangent_hit_requires_the_first_root() {
        let r = t::CUP_RADIUS_M;
        let from = Vec3::new(-r, 0.0, r - 1e-9);
        let to = Vec3::new(r, 0.0, from.z);
        let first = cup::rim_crossing(from, to, Vec3::ZERO);
        assert!(matches_rim_crossing(from, to, Vec3::ZERO, first));
        assert!(!matches_rim_crossing(from, to, Vec3::ZERO, Real::INFINITY));
        assert!(!matches_rim_crossing(from, to, Vec3::ZERO, 1.0 - first));
    }

    #[test]
    fn a_rounded_tangent_can_disagree_about_a_rim_crossing() {
        let radius = t::CUP_RADIUS_M;
        let from = Vec3::new(-radius, 0.0, radius + 1e-9);
        let to = Vec3::new(radius, 0.0, from.z);
        assert_eq!(cup::rim_crossing(from, to, Vec3::ZERO), Real::INFINITY);
        assert!(grazing_rim(from, to, Vec3::ZERO, 0.5).is_some());

        let miss = Vec3::new(0.0, 0.0, 1e-4);
        assert!(grazing_rim(from + miss, to + miss, Vec3::ZERO, 0.5).is_none());
        assert!(grazing_rim(from, to, Vec3::ZERO, 0.2).is_none());
    }
}
