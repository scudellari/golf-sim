use std::f64::consts::{FRAC_PI_2, PI, TAU};

use golf_sim_base::{
    Basis, Real, Seed, Vec2, Vec3, Vec4, fm, num,
    seed::SeedStream,
    variant::{Array, Variant},
};

use crate::determinism::Item;

const SAMPLE_SEED: Seed = Seed(-0x0123_4567_89ab_cdef);
const SAMPLES: usize = 256;
const SUBNORMAL: Real = Real::MIN_POSITIVE * Real::EPSILON;
const EDGES: &[Real] = &[
    -Real::MAX,
    -1e300,
    -1075.0,
    -1074.0,
    -1022.0,
    -746.0,
    -745.0,
    -710.0,
    -TAU,
    -PI,
    -FRAC_PI_2,
    -1.0 - Real::EPSILON,
    -1.0,
    -1.0 + Real::EPSILON / 2.0,
    -0.5,
    -Real::EPSILON,
    -Real::MIN_POSITIVE,
    -SUBNORMAL,
    -0.0,
    0.0,
    SUBNORMAL,
    Real::MIN_POSITIVE,
    Real::EPSILON,
    0.5,
    1.0 - Real::EPSILON / 2.0,
    1.0,
    1.0 + Real::EPSILON,
    FRAC_PI_2,
    PI,
    TAU,
    709.0,
    710.0,
    1023.0,
    1024.0,
    1e300,
    Real::MAX,
];

pub fn items() -> Vec<Item> {
    vec![
        Item::new("fm.acos", || unit(fm::acos)),
        Item::new("fm.acosh", || {
            array(samples(1.0, 1024.0).filter(|&x| x >= 1.0).map(fm::acosh))
        }),
        Item::new("fm.asin", || unit(fm::asin)),
        Item::new("fm.asinh", || unary(fm::asinh)),
        Item::new("fm.atan", || unary(fm::atan)),
        Item::new("fm.atan2", || binary(fm::atan2)),
        Item::new("fm.atanh", || unit(fm::atanh)),
        Item::new("fm.cbrt", || unary(fm::cbrt)),
        Item::new("fm.ceil", || unary(fm::ceil)),
        Item::new("fm.cos", || unary(fm::cos)),
        Item::new("fm.cosh", || unary(fm::cosh)),
        Item::new("fm.exp", || unary(fm::exp)),
        Item::new("fm.exp2", || unary(fm::exp2)),
        Item::new("fm.expm1", || unary(fm::expm1)),
        Item::new("fm.floor", || unary(fm::floor)),
        Item::new("fm.fmod", || {
            array(
                pairs()
                    .filter(|&(_, y)| y != 0.0)
                    .map(|(x, y)| fm::fmod(x, y)),
            )
        }),
        Item::new("fm.hypot", || binary(fm::hypot)),
        Item::new("fm.log", || nonnegative(fm::log)),
        Item::new("fm.log1p", || {
            array(samples(-1.0, 1024.0).filter(|&x| x >= -1.0).map(fm::log1p))
        }),
        Item::new("fm.log2", || nonnegative(fm::log2)),
        Item::new("fm.log10", || nonnegative(fm::log10)),
        Item::new("fm.pow", powers),
        Item::new("fm.round", || unary(fm::round)),
        Item::new("fm.sin", || unary(fm::sin)),
        Item::new("fm.sincos", || {
            array(samples(-1024.0, 1024.0).flat_map(|x| {
                let (sin, cos) = fm::sincos(x);
                [sin, cos]
            }))
        }),
        Item::new("fm.sinh", || unary(fm::sinh)),
        Item::new("fm.sqrt", || nonnegative(fm::sqrt)),
        Item::new("fm.tan", || unary(fm::tan)),
        Item::new("fm.tanh", || unary(fm::tanh)),
        Item::new("num.basis", basis),
        Item::new("num.builtins", builtins),
        Item::new("num.posmod", posmod),
        Item::new("num.roundi", || {
            array(samples(-1024.0, 1024.0).map(num::roundi))
        }),
        Item::new("num.vec2", vec2),
        Item::new("num.vec3", vec3),
        Item::new("num.vec3.is_finite", vec3_finiteness),
        Item::new("num.vec4", vec4),
        Item::new("seed.derive", derivations),
        Item::new("seed.derived", || {
            let mut stream = SeedStream::derived(SAMPLE_SEED, "風/⛳");
            array((0..SAMPLES).map(|_| stream.next_int()))
        }),
        Item::new("seed.float", || stream(SeedStream::next_float)),
        Item::new("seed.gaussian", || stream(SeedStream::next_gaussian)),
        Item::new("seed.int", || stream(SeedStream::next_int)),
        Item::new("seed.range", || stream(|rng| rng.next_range(-1e6, 1e-6))),
        Item::new("seed.sign", || stream(SeedStream::next_sign)),
    ]
}

fn array<I: Into<Variant>>(values: impl IntoIterator<Item = I>) -> Variant {
    Variant::Array(Array::new(values.into_iter().map(Into::into).collect()))
}

fn samples(low: Real, high: Real) -> impl Iterator<Item = Real> {
    let mut rng = SeedStream::new(SAMPLE_SEED);
    EDGES
        .iter()
        .copied()
        .chain((0..SAMPLES).map(move |_| rng.next_range(low, high)))
}

fn unary(op: fn(Real) -> Real) -> Variant {
    array(samples(-1024.0, 1024.0).map(op))
}

// Hash valid-domain outputs so platform-specific NaN payloads never enter a fixture.
fn unit(op: fn(Real) -> Real) -> Variant {
    array(
        samples(-1.0, 1.0)
            .filter(|x| (-1.0..=1.0).contains(x))
            .map(op),
    )
}

fn nonnegative(op: fn(Real) -> Real) -> Variant {
    array(samples(0.0, 1024.0).filter(|&x| x >= 0.0).map(op))
}

fn pairs() -> impl Iterator<Item = (Real, Real)> {
    let edges = [
        (-0.0, -0.0),
        (-0.0, 0.0),
        (0.0, -0.0),
        (0.0, 0.0),
        (Real::MAX, SUBNORMAL),
    ];
    let mut rng = SeedStream::new(SAMPLE_SEED);
    edges
        .into_iter()
        .chain(EDGES.iter().copied().zip(EDGES.iter().rev().copied()))
        .chain((0..SAMPLES).map(move |_| {
            (
                rng.next_range(-1024.0, 1024.0),
                rng.next_range(-1024.0, 1024.0),
            )
        }))
}

fn binary(op: fn(Real, Real) -> Real) -> Variant {
    array(pairs().map(|(x, y)| op(x, y)))
}

fn powers() -> Variant {
    let edges = [
        (-2.0, -3.0),
        (-2.0, 3.0),
        (-2.0, 4.0),
        (-0.0, 3.0),
        (0.0, -1.0),
        (0.0, 0.0),
        (SUBNORMAL, 0.5),
        (1.0 - Real::EPSILON / 2.0, 1e16),
        (1.0 + Real::EPSILON, 1e16),
        (2.0, -1074.0),
        (2.0, 1024.0),
        (Real::MAX, 0.5),
    ];
    let mut rng = SeedStream::new(SAMPLE_SEED);
    array(
        edges
            .into_iter()
            .chain((0..SAMPLES).map(|_| (rng.next_range(0.0, 16.0), rng.next_range(-8.0, 8.0))))
            .map(|(base, exponent)| fm::pow(base, exponent)),
    )
}

fn builtins() -> Variant {
    let mut rng = SeedStream::new(SAMPLE_SEED);
    let mut values = vec![
        num::smoothstep(0.0, 0.0, 0.0),
        num::smoothstep(1e-6, 0.0, 1e-6),
        num::wrapf(3.0, 1.0, 3.0),
        num::wrapf(3.0, 1.0, 1.0),
        num::snappedf(-1.5, 1.0),
        num::snappedf(1.5, 1.0),
        num::snappedf(-0.5, 0.0),
    ];
    for _ in 0..SAMPLES {
        let low = rng.next_range(-32.0, -1.0);
        let high = rng.next_range(1.0, 32.0);
        let value = rng.next_range(-48.0, 48.0);
        let weight = rng.next_range(-1.0, 2.0);
        let step = rng.next_range(0.125, 4.0) * rng.next_sign();
        values.extend([
            num::lerpf(low, high, weight),
            num::clampf(value, low, high),
            num::smoothstep(low, high, value),
            num::smoothstep(high, low, value),
            num::inverse_lerp(low, high, value),
            num::lerp_angle(low, high, weight),
            num::fposmod(value, step),
            num::wrapf(value, low, high),
            num::snappedf(value, step),
            num::deg_to_rad(value),
            num::rad_to_deg(value),
            num::roundf(value),
        ]);
    }
    array(values)
}

fn posmod() -> Variant {
    let mut rng = SeedStream::new(SAMPLE_SEED);
    let edges = [
        (i64::MIN, -1),
        (i64::MIN, 2),
        (i64::MAX, i64::MIN),
        (-1, 2),
        (1, -2),
    ];
    array(
        edges
            .into_iter()
            .chain((0..SAMPLES).map(|_| (rng.next_int(), rng.next_int() | 1)))
            .map(|(value, modulus)| num::posmod(value, modulus)),
    )
}

fn coordinates<const N: usize>(rng: &mut SeedStream) -> [Real; N] {
    std::array::from_fn(|_| rng.next_range(-32.0, 32.0))
}

fn vec2() -> Variant {
    let mut rng = SeedStream::new(SAMPLE_SEED);
    let mut values = Vec::new();
    for i in 0..SAMPLES {
        let [x, y] = coordinates(&mut rng);
        let a = if i == 0 { Vec2::ZERO } else { Vec2::new(x, y) };
        let [x, y] = coordinates(&mut rng);
        let b = Vec2::new(x, y);
        let factor = rng.next_range(0.25, 4.0);
        let angle = rng.next_range(-TAU, TAU);
        let limit = rng.next_range(0.0, 48.0);
        values.extend([
            a.dot(b),
            a.cross(b),
            a.length_squared(),
            a.length(),
            a.distance_to(b),
            a.angle_to(b),
        ]);
        values.extend(
            [
                a - b,
                a * factor,
                a / factor,
                a.normalized(),
                a.lerp(b, factor),
                a.slide(b.normalized()),
                a.limit_length(limit),
                a.rotated(angle),
                Vec2::from_angle(angle),
            ]
            .into_iter()
            .flat_map(|Vec2 { x, y }| [x, y]),
        );
    }
    array(values)
}

fn sample_vec3(rng: &mut SeedStream) -> Vec3 {
    let [x, y, z] = coordinates(rng);
    Vec3::new(x, y, z)
}

fn components3(Vec3 { x, y, z }: Vec3) -> [Real; 3] {
    [x, y, z]
}

fn vec3() -> Variant {
    let mut rng = SeedStream::new(SAMPLE_SEED);
    let mut values = Vec::new();
    for i in 0..SAMPLES {
        let a = if i == 0 {
            Vec3::ZERO
        } else {
            sample_vec3(&mut rng)
        };
        let b = sample_vec3(&mut rng);
        let axis = sample_vec3(&mut rng).normalized();
        let factor = rng.next_range(0.25, 4.0);
        let angle = rng.next_range(-TAU, TAU);
        let limit = rng.next_range(0.0, 64.0);
        let mut sum = a;
        sum += b;
        let mut difference = a;
        difference -= b;
        values.extend([
            a.dot(b),
            a.length_squared(),
            a.length(),
            a.distance_to(b),
            a.angle_to(b),
            a.signed_angle_to(b, axis),
        ]);
        values.extend(
            [
                a + b,
                a - b,
                -a,
                a * factor,
                a / factor,
                sum,
                difference,
                a.cross(b),
                a.normalized(),
                a.lerp(b, factor),
                a.slide(axis),
                a.limit_length(limit),
                a.cdiv(b),
                a.rotated(axis, angle),
            ]
            .into_iter()
            .flat_map(components3),
        );
    }
    array(values)
}

fn vec3_finiteness() -> Variant {
    let mut rng = SeedStream::new(SAMPLE_SEED);
    let edges = [
        Vec3::new(Real::INFINITY, 0.0, 0.0),
        Vec3::new(0.0, Real::NEG_INFINITY, 0.0),
        Vec3::new(0.0, 0.0, Real::NAN),
    ];
    array(
        edges
            .into_iter()
            .chain((0..SAMPLES).map(|_| sample_vec3(&mut rng)))
            .map(|v| i64::from(v.is_finite())),
    )
}

fn vec4() -> Variant {
    let mut rng = SeedStream::new(SAMPLE_SEED);
    let mut values = Vec::new();
    for i in 0..SAMPLES {
        let [x, y, z, w] = coordinates(&mut rng);
        let a = if i == 0 {
            Vec4::ZERO
        } else {
            Vec4::new(x, y, z, w)
        };
        let [x, y, z, w] = coordinates(&mut rng);
        let b = Vec4::new(x, y, z, w);
        let factor = rng.next_range(-2.0, 2.0);
        values.extend([a.dot(b), a.length_squared(), a.length()]);
        values.extend(
            [a - b, a.normalized(), a.lerp(b, factor), a.scale(factor)]
                .into_iter()
                .flat_map(|Vec4 { x, y, z, w }| [x, y, z, w]),
        );
    }
    array(values)
}

fn basis() -> Variant {
    let mut rng = SeedStream::new(SAMPLE_SEED);
    let mut values = Vec::new();
    for _ in 0..SAMPLES {
        let axis = sample_vec3(&mut rng).normalized();
        let angle = rng.next_range(-TAU, TAU);
        let rotation = Basis::from_axis_angle(axis, angle);
        let [x, y, z] = rotation.rows;
        let transform = Basis {
            rows: [x * 2.0, y * 3.0, z * 4.0],
        };
        values.extend(
            [rotation, transform.inverse()]
                .into_iter()
                .flat_map(|basis| basis.rows)
                .flat_map(components3),
        );
        values.extend(components3(transform.xform(sample_vec3(&mut rng))));
    }
    array(values)
}

fn derivations() -> Variant {
    let seeds = [Seed(0), SAMPLE_SEED, Seed(i64::MIN), Seed(i64::MAX)];
    let labels = [
        "",
        "wind",
        "wind/gust",
        "café",
        "cafe\u{301}",
        "風/⛳",
        "a\0b",
    ];
    array(
        seeds
            .into_iter()
            .flat_map(|seed| labels.map(|label| seed.derive(label).0)),
    )
}

fn stream<I: Into<Variant>>(mut next: impl FnMut(&mut SeedStream) -> I) -> Variant {
    let mut rng = SeedStream::new(SAMPLE_SEED);
    array((0..SAMPLES).map(|_| next(&mut rng)))
}
