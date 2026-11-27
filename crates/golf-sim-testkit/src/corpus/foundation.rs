use golf_sim_base::{
    Basis, Real, Seed, Vec2, Vec3, Vec4, corpus,
    digest::object,
    fm,
    geometry::{get_closest_point_to_segment, is_point_in_polygon, segment_intersects_segment},
    num::{deg_to_rad, lerpf, rad_to_deg, smoothstep},
    seed::SeedStream,
    variant::{Components, Dict, FloatWidth, Variant},
};

use super::{
    Result, Runner,
    projection::{Pair, id},
    rules::{array, integer, items, outline, point_value},
};

const EDGE_TOLERANCE_M: Real = 1e-6;
const REFERENCE_POSITION_EPSILON: Real = 4.0 * f32::EPSILON as Real;

pub(super) fn project(runner: Runner, case: &Dict, drift: bool) -> Result<Pair> {
    let expected = field(case, "out");
    let op = field(case, "op").as_str().unwrap();
    match runner {
        Runner::MathPrims => math(case, op, expected, drift),
        Runner::SeedStreams => seed(case, op, expected, drift),
        Runner::GeometryCases => geometry(case, op, expected, drift),
        _ => unreachable!("earlier fixture format"),
    }
}

fn math(case: &Dict, op: &str, expected: &Variant, drift: bool) -> Result<Pair> {
    let a = field(case, "a");
    let b = field(case, "b");
    if op.starts_with("f_") {
        let (a, b, c) = (scalar(a), scalar(b), scalar(field(case, "c")));
        let actual = match op {
            "f_lerpf" => lerpf(a, b, c),
            "f_smoothstep" => smoothstep(a, b, c),
            "f_exp" => fm::exp(a),
            "f_pow" => fm::pow(a, b),
            "f_log" => fm::log(a),
            "f_atan2" => fm::atan2(a, b),
            "f_sqrt" => fm::sqrt(a),
            "f_cos" => fm::cos(a),
            "f_sin" => fm::sin(a),
            "f_deg_to_rad" => deg_to_rad(a),
            "f_rad_to_deg" => rad_to_deg(a),
            _ => return Err(format!("{}: unknown scalar operation {op}", id(case)).into()),
        };
        return Ok((expected.clone(), actual.into()));
    }

    let (actual, a_length, b_length, parameter, out_length) = if op == "v4_dot" {
        let (a, b) = (vec4(a), vec4(b));
        (
            a.dot(b).into(),
            a.length(),
            b.length(),
            0.0,
            scalar(expected).abs(),
        )
    } else if op.starts_with("v2_") {
        let (a, b, s) = (vec2(a), vec2(b), scalar(field(case, "s")));
        let pack = |value| Vec2::new(value, 0.0);
        let actual = match op {
            "v2_dot" => pack(a.dot(b)),
            "v2_length" => pack(a.length()),
            "v2_length_squared" => pack(a.length_squared()),
            "v2_normalized" => a.normalized(),
            "v2_limit_length" => a.limit_length(s),
            "v2_distance_to" => pack(a.distance_to(b)),
            _ => return Err(format!("{}: unknown Vector2 operation {op}", id(case)).into()),
        };
        (
            vector2(actual),
            a.length(),
            b.length(),
            s,
            vec2(expected).length(),
        )
    } else {
        let (a, b, s) = (vec3(a), vec3(b), scalar(field(case, "s")));
        let pack = |value| Vec3::new(value, 0.0, 0.0);
        let actual = match op {
            "dot" => pack(a.dot(b)),
            "cross" => a.cross(b),
            "length" => pack(a.length()),
            "length_squared" => pack(a.length_squared()),
            "normalized" => a.normalized(),
            "lerp" => a.lerp(b, s),
            "slide" => a.slide(b),
            "limit_length" => a.limit_length(s),
            "distance_to" => pack(a.distance_to(b)),
            "rotated" => a.rotated(b, s),
            "mul" => a * s,
            "div" => a / s,
            "basis_inv_xform" => Basis::from_axis_angle(b, s).inverse().xform(a),
            _ => return Err(format!("{}: unknown Vector3 operation {op}", id(case)).into()),
        };
        (
            point_value(actual),
            a.length(),
            b.length(),
            s,
            vec3(expected).length(),
        )
    };
    // A rounded product can cancel in the reference and leave a result smaller than its error.
    let scale = 1.0_f64
        .max(a_length)
        .max(b_length)
        .max(a_length * b_length)
        .max(parameter.abs() * a_length)
        .max(out_length);
    let bound = 8.0 * Real::from(f32::EPSILON) * scale;
    if drift {
        return Ok((expected.clone(), actual));
    }
    check_bound(case, expected, &actual, bound)?;
    Ok((expected.clone(), expected.clone()))
}

fn seed(case: &Dict, op: &str, expected: &Variant, drift: bool) -> Result<Pair> {
    if op == "derive" {
        let parent = Seed(integer(field(case, "parent")));
        let label = field(case, "label").as_str().unwrap();
        return Ok((expected.clone(), parent.derive(label).0.into()));
    }
    if op != "stream" {
        return Err(format!("{}: unknown seed operation {op}", id(case)).into());
    }
    let mut stream = SeedStream::new(Seed(integer(field(case, "seed"))));
    let calls = items(field(case, "calls"));
    let expected = items(expected);
    if calls.len() != expected.len() {
        return Err(format!("{}: stream output count differs", id(case)).into());
    }
    let mut oracle = Vec::new();
    let mut core = Vec::new();
    for (call, expected) in calls.iter().zip(expected) {
        let call = call.as_dict().unwrap();
        let op = field(call, "op").as_str().unwrap();
        let actual: Variant = match op {
            "next_int" => stream.next_int().into(),
            "next_float" => stream.next_float().into(),
            "next_range" => stream
                .next_range(scalar(field(call, "low")), scalar(field(call, "high")))
                .into(),
            "next_gaussian" => stream.next_gaussian().into(),
            _ => return Err(format!("{}: unknown stream operation {op}", id(case)).into()),
        };
        let actual = if op == "next_gaussian" && !drift {
            check_bound(
                case,
                expected,
                &actual,
                1e-12 * scalar(expected).abs().max(1.0),
            )?;
            expected.clone()
        } else {
            actual
        };
        oracle.push(object([(op, expected.clone())]));
        core.push(object([(op, actual)]));
    }
    Ok((array(oracle), array(core)))
}

fn geometry(case: &Dict, op: &str, expected: &Variant, drift: bool) -> Result<Pair> {
    let point = |key| vec2(field(case, key));
    let (actual, bound, exception) = match op {
        "is_point_in_polygon" => {
            let polygon = outline(field(case, "polygon"));
            let query = point("point");
            let actual = Variant::Bool(is_point_in_polygon(query, &polygon));
            let exception = (&actual != expected && near_edge(query, &polygon))
                .then_some("within 1 µm of a polygon edge");
            (actual, 0.0, exception)
        }
        "get_closest_point_to_segment" => {
            let [query, a, b] = ["point", "a", "b"].map(point);
            (
                vector2(get_closest_point_to_segment(query, a, b)),
                position_tolerance(&[query, a, b]),
                None,
            )
        }
        "segment_intersects_segment" => {
            let [a, b, c, d] = ["a", "b", "c", "d"].map(point);
            let bound = position_tolerance(&[a, b, c, d]);
            let actual = segment_intersects_segment(a, b, c, d);
            let crossing = match (actual, expected) {
                (Some(crossing), Variant::Nil) => Some(crossing),
                (None, Variant::Vector2(_)) => Some(vec2(expected)),
                _ => None,
            };
            let exception = crossing
                .filter(|&crossing| {
                    crossing_near_endpoint(crossing, a, b, bound)
                        || crossing_near_endpoint(crossing, c, d, bound)
                })
                .map(|_| "within the crossing endpoint tolerance");
            (actual.map_or(Variant::Nil, vector2), bound, exception)
        }
        _ => return Err(format!("{}: unknown geometry operation {op}", id(case)).into()),
    };
    if let Some(reason) = exception {
        println!("{} {op}: {reason}", id(case));
    }
    if drift {
        return Ok((expected.clone(), actual));
    }
    if exception.is_none() {
        check_bound(case, expected, &actual, bound)?;
        if let (Variant::Vector2(_), Variant::Vector2(_)) = (expected, &actual)
            && (vec2(expected) - vec2(&actual)).length_squared() > bound * bound
        {
            return Err(
                format!("{}: {op} exceeds the {bound:e} m position bound", id(case)).into(),
            );
        }
    }
    Ok((expected.clone(), expected.clone()))
}

fn check_bound(case: &Dict, expected: &Variant, actual: &Variant, absolute: Real) -> Result<()> {
    if let Some(path) = corpus::first_difference_with(expected, actual, |_| {
        Some(corpus::Tolerance {
            absolute,
            relative: 0.0,
        })
    }) {
        return Err(format!("{}: output {path} exceeds the {absolute:e} bound", id(case)).into());
    }
    Ok(())
}

fn field<'a>(case: &'a Dict, key: &str) -> &'a Variant {
    case.get(key).unwrap_or_else(|| panic!("missing {key}"))
}

fn scalar(value: &Variant) -> Real {
    let Variant::Float(value) = value else {
        panic!("expected a float, got {value:?}");
    };
    value.value()
}

fn vec2(value: &Variant) -> Vec2 {
    let Variant::Vector2(value) = value else {
        panic!("expected Vector2, got {value:?}");
    };
    let [x, y] = value.values;
    Vec2::new(x, y)
}

fn vec3(value: &Variant) -> Vec3 {
    let Variant::Vector3(value) = value else {
        panic!("expected Vector3, got {value:?}");
    };
    let [x, y, z] = value.values;
    Vec3::new(x, y, z)
}

fn vec4(value: &Variant) -> Vec4 {
    let Variant::Vector4(value) = value else {
        panic!("expected Vector4, got {value:?}");
    };
    let [x, y, z, w] = value.values;
    Vec4::new(x, y, z, w)
}

fn vector2(vector: Vec2) -> Variant {
    Variant::Vector2(Components {
        values: [vector.x, vector.y],
        width: FloatWidth::Double,
    })
}

fn position_tolerance(inputs: &[Vec2]) -> Real {
    let scale = inputs
        .iter()
        .fold(1.0_f64, |scale, p| scale.max(p.x.abs()).max(p.y.abs()));
    REFERENCE_POSITION_EPSILON * scale
}

fn crossing_near_endpoint(crossing: Vec2, a: Vec2, b: Vec2, tolerance: Real) -> bool {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let length_squared = dx * dx + dy * dy;
    let projection = (crossing.x - a.x) * dx + (crossing.y - a.y) * dy;
    let endpoint_delta = projection.abs().min((projection - length_squared).abs());
    // Compare |t - endpoint| <= tolerance / length without taking a square root.
    length_squared > 0.0
        && endpoint_delta * endpoint_delta <= tolerance * tolerance * length_squared
}

fn near_edge(query: Vec2, polygon: &[Vec2]) -> bool {
    polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1))
        .any(|(&a, &b)| {
            (query - get_closest_point_to_segment(query, a, b)).length_squared()
                <= EDGE_TOLERANCE_M * EDGE_TOLERANCE_M
        })
}
