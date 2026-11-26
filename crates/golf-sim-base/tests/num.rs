use std::{
    collections::BTreeMap,
    f64::consts::{FRAC_PI_2, PI, TAU},
    hint::black_box,
    path::Path,
};

use golf_sim_base::{
    Basis, Real, Vec2, Vec3, Vec4, corpus, fm,
    num::{
        clampf, deg_to_rad, fposmod, inverse_lerp, lerp_angle, lerpf, posmod, rad_to_deg, roundf,
        roundi, smoothstep, snappedf, wrapf,
    },
    variant::{Dict, Variant},
};

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

fn vector_error<const N: usize>(
    a_length: Real,
    b_length: Real,
    s: Real,
    expected: [Real; N],
    actual: [Real; N],
) -> Real {
    let out_length = fm::sqrt(expected.iter().map(|v| v * v).sum());
    // Rounded products can cancel in the reference, leaving a result smaller than their error.
    let scale = 1.0_f64
        .max(a_length)
        .max(b_length)
        .max(a_length * b_length)
        .max(s.abs() * a_length)
        .max(out_length);
    let bound = 8.0 * Real::from(f32::EPSILON) * scale;
    let mut worst: Real = 0.0;
    for (expected, actual) in expected.into_iter().zip(actual) {
        let error = (actual - expected).abs() / bound;
        if !error.is_finite() {
            return Real::INFINITY;
        }
        worst = worst.max(error);
    }
    worst
}

fn ulp_distance(expected: Real, actual: Real) -> u64 {
    let ordered_bits = |value: Real| {
        let sign = 1_u64 << 63;
        let bits = value.to_bits();
        if bits & sign == 0 {
            sign + bits
        } else {
            sign - (bits & !sign)
        }
    };
    if expected == actual {
        0
    } else if expected.is_finite() && actual.is_finite() {
        ordered_bits(expected).abs_diff(ordered_bits(actual))
    } else {
        u64::MAX
    }
}

#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
enum Error {
    Vector(Real),
    Scalar(u64),
}

impl Error {
    fn within_tolerance(self) -> bool {
        match self {
            Self::Vector(bound_multiple) => bound_multiple <= 1.0,
            Self::Scalar(ulps) => ulps <= 4,
        }
    }
}

fn case_error(op: &str, case: &Dict) -> Error {
    let a = field(case, "a");
    let b = field(case, "b");
    let out = field(case, "out");
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
            _ => panic!("unknown scalar operation {op}"),
        };
        Error::Scalar(ulp_distance(scalar(out), actual))
    } else if op == "v4_dot" {
        let (a, b) = (vec4(a), vec4(b));
        Error::Vector(vector_error(
            a.length(),
            b.length(),
            0.0,
            [scalar(out)],
            [a.dot(b)],
        ))
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
            _ => panic!("unknown Vector2 operation {op}"),
        };
        let expected = vec2(out);
        Error::Vector(vector_error(
            a.length(),
            b.length(),
            s,
            [expected.x, expected.y],
            [actual.x, actual.y],
        ))
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
            _ => panic!("unknown Vector3 operation {op}"),
        };
        let expected = vec3(out);
        Error::Vector(vector_error(
            a.length(),
            b.length(),
            s,
            [expected.x, expected.y, expected.z],
            [actual.x, actual.y, actual.z],
        ))
    }
}

#[test]
fn math_prims_match_the_reference() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/math/prims/fixture.bin");
    let fixture = corpus::read(&path).unwrap();
    let fixture = fixture.as_dict().unwrap();
    assert_eq!(
        corpus::first_difference(
            &Variant::String("math/prims".into()),
            field(fixture, "layer")
        ),
        None
    );
    let Variant::Array(cases) = field(fixture, "cases") else {
        panic!("expected math/prims cases");
    };

    let mut results = BTreeMap::new();
    let mut failures = Vec::new();
    for case in &cases.items {
        let case = case.as_dict().unwrap();
        let id = field(case, "id").as_str().unwrap();
        let op = field(case, "op").as_str().unwrap();
        let error = case_error(op, case);
        assert!(
            !matches!(error, Error::Vector(bound_multiple) if !bound_multiple.is_finite()),
            "{op} {id}: {error:?}"
        );
        if !error.within_tolerance() {
            failures.push((id, error));
        }
        let (count, worst) = results.entry(op).or_insert((0, error));
        *count += 1;
        if error > *worst {
            *worst = error;
        }
    }

    assert_eq!(results.len(), 31);
    for (op, (count, worst)) in results {
        assert_eq!(count, 1_000, "{op}");
        match worst {
            Error::Vector(bound_multiple) => println!("{op:<16} {bound_multiple:.9e} × bound"),
            Error::Scalar(ulps) => println!("{op:<16} {ulps} ULP"),
        }
    }
    assert!(
        failures.is_empty(),
        "{} cases out of tolerance: {failures:?}",
        failures.len()
    );
}

#[test]
fn lerp_multiply_and_add_round_separately() {
    let delta = 1.0 / 134_217_728.0;
    let (from, to, weight) = black_box((-1.0, delta, 1.0 - delta));
    let result = lerpf(from, to, weight);
    let fused_result = -delta * delta;
    println!("lerpf gives {result:e}; a fused evaluation gives {fused_result:e}");
    assert_eq!(result.to_bits(), 0);
    assert_ne!(result, fused_result);
}

#[test]
fn scalar_ranges() {
    assert_eq!(clampf(-3.0, -2.0, 2.0), -2.0);
    assert_eq!(clampf(3.0, -2.0, 2.0), 2.0);
    assert_eq!(clampf(-1.0, -2.0, 2.0), -1.0);
    assert_eq!(clampf(2.0, 2.0, 2.0), 2.0);

    assert_eq!(inverse_lerp(-4.0, 0.0, -4.0), 0.0);
    assert_eq!(inverse_lerp(-4.0, 0.0, 0.0), 1.0);
    assert_eq!(inverse_lerp(-4.0, 0.0, -5.0), -0.25);
    assert_eq!(inverse_lerp(0.0, -4.0, -3.0), 0.75);

    assert_eq!(smoothstep(-4.0, 0.0, -3.0), 0.15625);
    assert_eq!(smoothstep(0.0, -4.0, -3.0), 0.84375);
    assert_eq!(smoothstep(-1.0, -1.0, -1.0), 0.0);
    assert_eq!(smoothstep(-1.0, -1.0, 0.0), 1.0);
    assert_eq!(smoothstep(1.0, 1.000001, 1.0000005), 1.0);
    assert_eq!(smoothstep(1.000001, 1.0, 1.0000005), 0.0);
}

#[test]
fn scalar_wrapping_and_rounding() {
    assert_eq!(fposmod(-5.5, 3.0), 0.5);
    assert_eq!(fposmod(5.5, -3.0), -0.5);
    assert_eq!(fposmod(-5.5, -3.0), -2.5);
    assert_eq!(fposmod(-0.0, 3.0).to_bits(), 0);
    assert_eq!(fposmod(-6.0, 3.0).to_bits(), 0);

    assert_eq!(posmod(-5, 3), 1);
    assert_eq!(posmod(5, -3), -1);
    assert_eq!(posmod(-5, -3), -2);
    assert_eq!(posmod(6, -3), 0);
    assert_eq!(posmod(i64::MIN, -1), 0);
    assert_eq!(posmod(1, i64::MIN), i64::MIN + 1);

    assert_eq!(wrapf(-3.0, -2.0, 2.0), 1.0);
    assert_eq!(wrapf(-2.0, -2.0, 2.0), -2.0);
    assert_eq!(wrapf(2.0, -2.0, 2.0), -2.0);
    assert_eq!(wrapf(7.0, 2.0, 2.0), 2.0);
    assert_eq!(wrapf(0.0000005, 0.0, 0.000001), 0.0);
    assert_eq!(wrapf(0.999995, 0.0, 1.0), 0.0);

    assert_eq!(snappedf(-0.75, 0.5), -0.5);
    assert_eq!(snappedf(0.75, 0.5), 1.0);
    assert_eq!(snappedf(0.75, -0.5), 0.5);
    assert_eq!(snappedf(-1.25, 0.0), -1.25);

    for (input, expected) in [(-1.5, -2), (-0.5, -1), (0.0, 0), (0.5, 1), (1.5, 2)] {
        assert_eq!(roundf(input), expected as Real);
        assert_eq!(roundi(input), expected);
    }
    assert_eq!(roundf(-0.25).to_bits(), (-0.0_f64).to_bits());
}

#[test]
fn angles_use_the_short_arc_and_the_axis_sign() {
    assert_eq!(lerp_angle(0.0, PI, 0.5), -FRAC_PI_2);
    assert_eq!(lerp_angle(0.0, -PI, 0.5), FRAC_PI_2);
    assert_eq!(lerp_angle(-TAU, 0.0, 1.0), -TAU);
    assert_eq!(lerp_angle(0.0, 1.5 * PI, 0.5), -PI / 4.0);

    let right = Vec2::new(1.0, 0.0);
    assert_eq!(Vec2::from_angle(0.0), right);
    let up = Vec2::new(0.0, -1.0);
    assert!((Vec2::from_angle(-FRAC_PI_2) - up).length() <= Real::EPSILON);
    assert!((right.rotated(-FRAC_PI_2) - up).length() <= Real::EPSILON);
    assert_eq!(right.angle_to(up), -FRAC_PI_2);
    assert_eq!(up.angle_to(right), FRAC_PI_2);

    let x = Vec3::new(1.0, 0.0, 0.0);
    let y = Vec3::new(0.0, 1.0, 0.0);
    let z = Vec3::new(0.0, 0.0, 1.0);
    assert_eq!(x.angle_to(y), FRAC_PI_2);
    assert_eq!(x.angle_to(-x), PI);
    assert_eq!(x.angle_to(Vec3::ZERO), 0.0);
    assert_eq!(x.signed_angle_to(y, -z), -FRAC_PI_2);
    assert_eq!(x.signed_angle_to(y, z), FRAC_PI_2);
    assert_eq!(x.signed_angle_to(-x, z), PI);
}

#[test]
fn basis_inverse_handles_scale_and_shear() {
    let basis = Basis {
        rows: [
            Vec3::new(-2.0, 2.0, 0.0),
            Vec3::new(0.0, 1.0, 1.0),
            Vec3::new(0.0, 0.0, 0.5),
        ],
    };
    assert_eq!(
        basis.inverse().rows,
        [
            Vec3::new(-0.5, 1.0, -2.0),
            Vec3::new(0.0, 1.0, -2.0),
            Vec3::new(0.0, 0.0, 2.0),
        ]
    );

    let x = Vec3::new(1.0, 0.0, 0.0);
    let y = Vec3::new(0.0, 1.0, 0.0);
    let z = Vec3::new(0.0, 0.0, 1.0);
    assert_eq!(Basis::from_axis_angle(z, 0.0).inverse().rows, [x, y, z]);
    let inverse = Basis::from_axis_angle(z, FRAC_PI_2).inverse();
    assert!((inverse.xform(x) + y).length() <= Real::EPSILON);
}

#[test]
fn vector_boundaries() {
    assert_eq!(Vec2::ZERO.normalized(), Vec2::ZERO);
    assert_eq!(Vec3::ZERO.normalized(), Vec3::ZERO);
    assert_eq!(Vec4::ZERO.normalized(), Vec4::ZERO);
    let v2 = Vec2::new(3.0, 4.0);
    assert_eq!(v2.limit_length(5.0), v2);
    assert_eq!(v2.slide(Vec2::new(0.0, 1.0)), Vec2::new(3.0, 0.0));
    assert_eq!(v2.lerp(Vec2::ZERO, 0.5), Vec2::new(1.5, 2.0));
    let v3 = Vec3::new(3.0, 4.0, 0.0);
    assert_eq!(v3.limit_length(0.0), Vec3::ZERO);
    assert_eq!(
        v3.cdiv(Vec3::new(-1.0, 2.0, 1.0)),
        Vec3::new(-3.0, 2.0, 0.0)
    );
    let v4 = Vec4::new(0.0, -2.0, 0.0, 0.0);
    assert_eq!(v4.length(), 2.0);
    assert_eq!(v4.normalized(), v4.scale(0.5));
    assert_eq!(v4.lerp(Vec4::ZERO, 0.5), v4 - v4.scale(0.5));
}
