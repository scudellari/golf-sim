use golf_sim_base::{
    Real, Vec2, Vec3,
    digest::object,
    fm,
    geometry::{get_closest_point_to_segment, is_point_in_polygon},
    variant::{Components, Dict, FloatWidth, Variant},
};
use golf_sim_course::{
    course::{Course, CourseData, LegalKind, PenaltyColor, Tree},
    legal::{Area, LegalMap, PenaltyArea, StanceFootprint},
    obstacles::ObstacleIndex,
    zones::ZoneMap,
};
use golf_sim_physics::values::{ObstacleKind, Zone};

const COURSE_STEPS: Real = 8.0;
const RAY_TOLERANCE_M: Real = 0.002;
const NO_RAY_HIT: Vec3 = Vec3::new(Real::INFINITY, Real::INFINITY, Real::INFINITY);

pub(super) fn grids(course: &Course, case: &Dict) -> Result<Variant, String> {
    let call = case.get("call").and_then(Variant::as_str).unwrap();
    let input = case.get("input").and_then(Variant::as_dict).unwrap();
    let position = || {
        let Some(Variant::Vector3(position)) = input.get("position") else {
            panic!("{call}: expected a Vector3 position");
        };
        let [x, y, z] = position.values;
        Vec3::new(x, y, z)
    };
    let plan = || {
        let Some(Variant::Vector2(plan)) = input.get("p") else {
            panic!("{call}: expected a Vector2 plan point");
        };
        let [x, z] = plan.values;
        Vec2::new(x, z)
    };

    Ok(match call {
        "height_at" => course.height_at(position()).into(),
        "ground_split_at" => {
            let s = course.ground_split_at(plan());
            Variant::Vector4(Components {
                values: [s.datum_m, s.offset_m, s.slope.x, s.slope.y],
                width: FloatWidth::Double,
            })
        }
        "normal_at" => {
            let normal = course.normal_at(position());
            Variant::Vector3(Components {
                values: [normal.x, normal.y, normal.z],
                width: FloatWidth::Double,
            })
        }
        "material_at" => course
            .material_at(position())
            .map_or(Variant::Nil, |material| {
                object([
                    ("id", material.id.as_str().into()),
                    ("kind", (material.kind as i64).into()),
                    ("grass_height_m", material.grass_height_m.into()),
                    ("grass", material.grass.as_str().into()),
                    ("soil", material.soil.as_str().into()),
                    ("drainage", material.drainage.as_str().into()),
                ])
            }),
        "material_index_at" => course
            .material_index_at(plan())
            .map_or(-1, |index| i64::from(index.0))
            .into(),
        _ => return Err(format!("unknown probe {call:?}")),
    })
}

pub(super) fn queries(
    queries: &Queries<'_>,
    case: &Dict,
    checks: &mut QueryChecks,
    drift: bool,
) -> Result<Variant, String> {
    let mut actual = queries.run(case)?;
    let expected = case.get("output").unwrap();
    let id = case.get("id").unwrap().as_str().unwrap();
    let call = case.get("call").and_then(Variant::as_str);
    let raw = (drift && matches!(call, Some("obstacle_hit" | "ray_hit"))).then(|| actual.clone());
    let before = checks.failure_count();
    if call == Some("obstacle_hit")
        && golf_sim_base::corpus::first_difference(expected, &actual).is_some()
    {
        let input = case.get("input").unwrap().as_dict().unwrap();
        let from = point3(input.get("from").unwrap());
        let to = point3(input.get("to").unwrap());
        let radius = scalar(input.get("radius").unwrap());
        let first = exact_solid_entry(queries.course.data(), from, to, radius);
        if !matches_solid_entry(first, from, to, &actual) {
            return Err("core obstacle result differs from the first valid solid entry".into());
        }
    }
    match call {
        Some("zone_at") => {
            let (Variant::Int(recorded), Variant::Int(computed)) = (expected, &actual) else {
                return Err("expected numeric zone codes".into());
            };
            if recorded.value() != computed.value() {
                let input = case.get("input").unwrap().as_dict().unwrap();
                let point = point3(input.get("position").unwrap());
                if let Some(gap) = zone_rounding_gap(
                    &queries.zone_polygons(),
                    Vec2::new(point.x, point.z),
                    recorded.value(),
                    computed.value(),
                ) {
                    println!(
                        "case {id}: zone_at reference boundary classification at {gap:.3e} m from the authored edge"
                    );
                    if !drift {
                        actual = expected.clone();
                    }
                }
            }
        }
        Some("legal.area_at" | "legal.penalty_at" | "zone_map.crossings")
            if golf_sim_base::corpus::first_difference(expected, &actual).is_some() =>
        {
            let input = case.get("input").unwrap().as_dict().unwrap();
            let reason = if call == Some("zone_map.crossings") {
                queries
                    .rounded_crossing(input, expected, &actual)
                    .then_some("reference zone classification on a vertex crossing")
            } else {
                queries
                    .rounded_penalty(input, expected, &actual)
                    .then_some("reference rounded the closest penalty edge onto the ball")
            };
            if let Some(reason) = reason {
                println!("case {id}: {reason}");
                if !drift {
                    actual = expected.clone();
                }
            }
        }
        Some("obstacle_hit") if matches!(actual, Variant::Nil) && expected.as_dict().is_some() => {
            if queries.rounded_trunk(case, expected.as_dict().unwrap()) {
                println!("case {id}: reference rounded a grazing trunk discriminant into a hit");
                if !drift {
                    actual = expected.clone();
                }
            }
        }
        Some("obstacle_hit") => {
            if let (Variant::Dict(expected), Variant::Dict(actual)) = (expected, &mut actual) {
                let input = case.get("input").unwrap().as_dict().unwrap();
                let endpoints = ["from", "to"].map(|key| point3(input.get(key).unwrap()));
                let scale = endpoints
                    .into_iter()
                    .flat_map(|p| [p.x, p.y, p.z])
                    .map(Real::abs)
                    .fold(1.0, Real::max);
                let step = scale * Real::from(f32::EPSILON);
                let bounds = trunk_bounds(
                    queries,
                    expected,
                    actual,
                    endpoints,
                    scalar(input.get("radius").unwrap()),
                    step,
                );
                let [point_limit, normal_limit] = bounds.unwrap_or([COURSE_STEPS; 2]);
                for (field, limit) in [("point", point_limit), ("normal", normal_limit)] {
                    checks.obstacle.vector(
                        id,
                        field,
                        expected.get(field).unwrap(),
                        actual.get_mut(field).unwrap(),
                        step,
                        limit,
                    );
                }
                if let (Variant::Float(expected), Variant::Float(actual)) = (
                    expected.get("fraction").unwrap(),
                    actual.get_mut("fraction").unwrap(),
                ) {
                    checks.obstacle.record(
                        id,
                        "fraction",
                        (actual.value() - expected.value()).abs() / step,
                        COURSE_STEPS,
                    );
                    *actual = *expected;
                }
            }
        }
        Some("ray_hit") => {
            let input = case.get("input").unwrap().as_dict().unwrap();
            let (expected_point, actual_point) = (point3(expected), point3(&actual));
            checks
                .ray
                .check(id, input, queries, expected_point, actual_point);
            // The independent entry check also covers hit presence.
            actual = expected.clone();
        }
        _ => {}
    }
    if !drift && checks.failure_count() > before {
        return Err("query outputs exceeded their geometric bounds".into());
    }
    Ok(raw.unwrap_or(actual))
}

fn trunk_bounds(
    queries: &Queries<'_>,
    expected: &Dict,
    actual: &Dict,
    [from, to]: [Vec3; 2],
    radius: Real,
    endpoint_step: Real,
) -> Option<[Real; 2]> {
    let trunk = |hit: &Dict| matches!(hit.get("kind"), Some(Variant::Int(kind)) if kind.value() == ObstacleKind::Trunk as i64);
    if !trunk(expected) || !trunk(actual) {
        return None;
    }
    let point = point3(actual.get("point")?);
    let recorded_point = point3(expected.get("point")?);
    let computed_normal = point3(actual.get("normal")?);
    let plan = |p: Vec3| Vec2::new(p.x, p.z);
    let tree = queries.course.data().trees.instances.iter().find(|tree| {
        let offset = plan(point - tree.base);
        (offset.length() - tree.trunk_radius_m - radius).abs() <= 1e-6
            && (offset.normalized() - plan(computed_normal)).length() <= 1e-6
    })?;
    let relative = plan(recorded_point - tree.base);
    let normal = point3(expected.get("normal")?);
    let precision = COURSE_STEPS * Real::from(f32::EPSILON);
    if (relative.normalized() - plan(normal)).length() > precision || normal.y != 0.0 {
        return None;
    }
    let direction = plan(to - from);
    let planar_length = direction.length();
    let from_center = plan(from - tree.base);
    let reach = radius + tree.trunk_radius_m;
    if planar_length == 0.0 || reach <= 0.0 {
        return None;
    }
    let unit = direction / planar_length;
    let across = from_center.cross(unit);
    let chord_squared = reach * reach - across * across;
    let shift = chord_rounding(
        chord_squared,
        precision * (from_center.length_squared() + reach * reach),
    );
    let along = shift + precision * from_center.length();
    let point_error = along * (to - from).length() / planar_length + COURSE_STEPS * endpoint_step;
    let normal_error = 2.0 * plan(recorded_point - point).length() / reach + precision;
    Some([
        COURSE_STEPS.max(point_error / endpoint_step),
        COURSE_STEPS.max(normal_error / endpoint_step),
    ])
}

fn chord_rounding(chord_squared: Real, error: Real) -> Real {
    let half_chord = fm::sqrt(chord_squared.max(0.0));
    (fm::sqrt((chord_squared + error).max(0.0)) - half_chord)
        .max(half_chord - fm::sqrt((chord_squared - error).max(0.0)))
}

#[derive(Default)]
pub(super) struct QueryChecks {
    obstacle: QueryErrors,
    ray: RayErrors,
}

impl QueryChecks {
    fn failure_count(&self) -> usize {
        self.obstacle.outside.len()
            + self.ray.core_ray.outside.len()
            + self.ray.surface.outside.len()
            + self.ray.along_ray.outside.len()
            + self.ray.kinds.len()
    }

    pub(super) fn finish(&self) -> Result<(), String> {
        self.obstacle
            .print("obstacle_hit", "single-precision steps at endpoint scale");
        let rays_pass = self.ray.report();
        if self.obstacle.outside.is_empty() && rays_pass {
            Ok(())
        } else {
            Err("query outputs exceeded tolerance".into())
        }
    }
}

#[derive(Default)]
struct QueryErrors {
    maximum: Option<(Real, String)>,
    outside: Vec<(Real, Real, String)>,
}

impl QueryErrors {
    fn vector(
        &mut self,
        id: &str,
        field: &str,
        expected: &Variant,
        actual: &mut Variant,
        unit: Real,
        limit: Real,
    ) {
        let (Variant::Vector3(expected), Variant::Vector3(actual)) = (expected, actual) else {
            return;
        };
        if !expected
            .values
            .into_iter()
            .chain(actual.values)
            .all(Real::is_finite)
        {
            return;
        }
        for ((axis, expected), actual) in ["x", "y", "z"]
            .into_iter()
            .zip(expected.values)
            .zip(&mut actual.values)
        {
            let error = (*actual - expected).abs() / unit;
            self.record(id, &format!("{field}.{axis}"), error, limit);
            // The shared check still verifies the output shape and all other fields.
            *actual = expected;
        }
    }

    fn record(&mut self, id: &str, field: &str, error: Real, limit: Real) {
        if !error.is_finite() || error > limit {
            self.outside.push((error, limit, format!("{id} {field}")));
        }
        if self.maximum.as_ref().is_none_or(|(max, _)| error > *max) {
            self.maximum = Some((error, format!("{id} {field}")));
        }
    }

    fn print(&self, call: &str, unit: &str) {
        if let Some((error, field)) = &self.maximum {
            println!("{call} maximum error: {error:.6} {unit} ({field})");
        }
        for (error, limit, field) in &self.outside {
            println!("{field}: {error:.6} {unit} (limit {limit:.6})");
        }
    }
}

#[derive(Default)]
struct RayErrors {
    reference_ray: QueryErrors,
    core_ray: QueryErrors,
    surface: QueryErrors,
    along_ray: QueryErrors,
    kinds: Vec<String>,
    hits: usize,
    misses: usize,
}

impl RayErrors {
    fn check(
        &mut self,
        id: &str,
        input: &Dict,
        queries: &Queries<'_>,
        expected: Vec3,
        actual: Vec3,
    ) {
        let origin = point3(input.get("origin").unwrap());
        let direction = point3(input.get("direction").unwrap()).normalized();
        let max = scalar(input.get("max_m").unwrap());
        let expected_along = (expected - origin).dot(direction);
        let actual_along = (actual - origin).dot(direction);
        let hit_limit = if actual.is_finite() {
            (actual_along + RAY_TOLERANCE_M).min(max)
        } else {
            max
        };
        let first = queries.first_ray_entry(origin, direction, hit_limit);
        let matches = match first {
            Some(first) => matches_ray_entry(actual_along, first),
            None => actual == NO_RAY_HIT,
        };
        if !matches {
            self.kinds.push(format!(
                "{id} core ray at {actual_along:.8} m differs from first entry {first:?} m"
            ));
            return;
        }
        if expected == NO_RAY_HIT && actual == NO_RAY_HIT {
            self.misses += 1;
            return;
        }
        let scale = [origin, expected]
            .into_iter()
            .filter(|v| v.is_finite())
            .flat_map(|v| [v.x.abs(), v.y.abs(), v.z.abs()])
            .fold(1.0, Real::max);
        let reference_departure = || {
            let (sampled, steps) = queries.sampled_ray(origin, direction, max);
            let rounding_m =
                Real::from(f32::EPSILON) * scale * (COURSE_STEPS + 0.5 * steps as Real);
            let same = sampled == expected
                || (sampled - expected).length() <= RAY_TOLERANCE_M + rounding_m;
            if same {
                println!(
                    "{id}: reference ray returned the march sample despite an earlier entry or its distance limit"
                );
            }
            same
        };
        if !expected.is_finite() || !actual.is_finite() {
            if !reference_departure() {
                self.kinds.push(format!(
                    "{id} hit presence differs: {expected:?} vs {actual:?}"
                ));
            }
            return;
        }
        self.hits += 1;
        // A 25 m march step refined 14 times resolves 1.53 mm, with additional drift
        // from the reference's single-precision positions.
        for (errors, point, along, limit) in [
            (
                &mut self.reference_ray,
                expected,
                expected_along,
                RAY_TOLERANCE_M,
            ),
            (
                &mut self.core_ray,
                actual,
                actual_along,
                64.0 * Real::EPSILON * scale,
            ),
        ] {
            let off_ray = (point - origin - direction * along.max(0.0)).length();
            errors.record(id, "point", off_ray * 1000.0, limit * 1000.0);
        }

        // A ray starting inside a crown returns the first refined point in the
        // crown. There is no surface between that point and the ray's origin.
        let first_refinement_m = 25.0 / 16_384.0;
        let origin_rounding_m = COURSE_STEPS
            * Real::from(f32::EPSILON)
            * origin
                .x
                .abs()
                .max(origin.y.abs())
                .max(origin.z.abs())
                .max(1.0);
        if (actual - origin).length() <= first_refinement_m
            && (expected - origin).length() <= first_refinement_m + origin_rounding_m
            && [origin, expected, actual]
                .into_iter()
                .all(|point| queries.obstacles.canopy_density(point) > 0.0)
        {
            return;
        }

        let expected_surface = queries.ray_surface(expected, direction);
        let surface = queries.ray_surface(actual, direction);
        if expected_surface.kind != surface.kind || expected_surface.gap_m > RAY_TOLERANCE_M {
            if reference_departure() {
                return;
            }
            self.kinds.push(format!(
                "{id} expected {expected_surface:?}, got {surface:?}"
            ));
        }
        self.surface.record(
            id,
            &format!("{:?}", surface.kind),
            surface.gap_m * 1000.0,
            RAY_TOLERANCE_M * 1000.0,
        );

        // Reproject the rounded hit onto the input ray using the surface tangent.
        // The remaining budget is measured along that ray.
        let sine = direction.dot(surface.normal);
        let march_steps = queries.sampled_ray(origin, direction, max).1;
        let world_step =
            (COURSE_STEPS + 0.5 * march_steps as Real) * Real::from(f32::EPSILON) * scale;
        let off_ray = expected - (origin + direction * expected_along);
        let (distance, allowance) = if sine.abs() > 0.0 {
            let reference_along = expected_along + off_ray.dot(surface.normal) / sine;
            (
                (reference_along - actual_along).abs(),
                RAY_TOLERANCE_M + world_step / sine.abs(),
            )
        } else {
            (
                (expected_along - actual_along).abs(),
                RAY_TOLERANCE_M + world_step,
            )
        };
        if distance > allowance
            && (expected_along - actual_along).abs() > RAY_TOLERANCE_M
            && reference_departure()
        {
            return;
        }
        self.along_ray
            .record(id, "point (along ray)", distance / allowance, 1.0);
    }

    fn report(&self) -> bool {
        // The reference's own drift off its ray is printed for context, never required: it
        // measures the reference's single-precision march, not the port.
        self.reference_ray.print("ray_hit reference off ray", "mm");
        self.core_ray.print("ray_hit core off ray", "mm");
        self.surface.print("ray_hit core surface gap", "mm");
        self.along_ray
            .print("ray_hit along ray", "of allowed error");
        println!(
            "ray_hit kinds: {} finite pairs, {} matching misses, {} differences",
            self.hits,
            self.misses,
            self.kinds.len(),
        );
        for difference in &self.kinds {
            println!("{difference}");
        }
        self.core_ray.outside.is_empty()
            && self.surface.outside.is_empty()
            && self.along_ray.outside.is_empty()
            && self.kinds.is_empty()
    }
}

#[derive(Debug, PartialEq, Eq)]
enum RaySurfaceKind {
    Ground,
    Obstacle(ObstacleKind),
    Canopy,
}

#[derive(Debug)]
struct RaySurface {
    kind: RaySurfaceKind,
    normal: Vec3,
    gap_m: Real,
}

fn crown_surface(tree: &Tree, point: Vec3) -> Option<RaySurface> {
    let height = tree.height_m - tree.crown_base_m;
    let radius = tree.crown_radius_m;
    if tree.density <= 0.0 || height <= 0.0 || radius <= 0.0 {
        return None;
    }
    let offset = point - tree.base;
    let up = Vec3::new(0.0, 1.0, 0.0);
    let y = offset.y - tree.crown_base_m;
    let across = Vec3::new(offset.x, 0.0, offset.z);
    let (normal, gap_m) = if tree.cone {
        let radial = across.length();
        if !(-RAY_TOLERANCE_M..=height + RAY_TOLERANCE_M).contains(&y) {
            return None;
        }
        let slope = radius / height;
        let side_gap = (radial - slope * (height - y)).abs() / fm::sqrt(1.0 + slope * slope);
        if radial <= radius && y.abs() < side_gap {
            (-up, y.abs())
        } else {
            ((across.normalized() + up * slope).normalized(), side_gap)
        }
    } else {
        let half = height * 0.5;
        let radii = Vec3::new(radius, half, radius);
        let scaled = (across + up * (y - half)).cdiv(radii);
        let gradient = scaled.cdiv(radii);
        // The ellipsoid's implicit residual divided by its gradient gives the local surface gap.
        let gap = (scaled.length_squared() - 1.0).abs() / (2.0 * gradient.length());
        (gradient.normalized(), gap)
    };
    (gap_m <= RAY_TOLERANCE_M).then_some(RaySurface {
        kind: RaySurfaceKind::Canopy,
        normal,
        gap_m,
    })
}

fn reference_zone_contains(point: Vec2, outline: &[Vec2]) -> bool {
    let in_bounds = |axis: fn(Vec2) -> Real| {
        let (lo, hi) = outline
            .iter()
            .copied()
            .map(axis)
            .fold((Real::INFINITY, Real::NEG_INFINITY), |(lo, hi), value| {
                (lo.min(value), hi.max(value))
            });
        (lo - 1e-6..hi + 1e-6).contains(&axis(point))
    };
    in_bounds(|p| p.x) && in_bounds(|p| p.y) && is_point_in_polygon(point, outline)
}

fn reference_edge_gap(point: Vec2, outline: &[Vec2]) -> Option<Real> {
    const COMPARE_EPSILON: Real = 1e-5;

    // The reference tests equality in world coordinates at the query point and
    // in edge-length units when deciding which side of an edge the ray is on.
    let coincidence = Vec2::new(point.x.abs().max(1.0), point.y.abs().max(1.0)).length();
    outline
        .iter()
        .zip(outline.iter().cycle().skip(1))
        .filter_map(|(&a, &b)| {
            let bound = COMPARE_EPSILON * (b - a).length().max(coincidence);
            let gap = (point - get_closest_point_to_segment(point, a, b)).length();
            (gap <= bound).then_some(gap)
        })
        .min_by(Real::total_cmp)
}

fn exact_polygon_contains(p: Vec2, outline: &[Vec2]) -> bool {
    let mut inside = false;
    for (&a, &b) in outline.iter().zip(outline.iter().cycle().skip(1)) {
        let (low, high) = if a.y < b.y { (a, b) } else { (b, a) };
        if (low.y..high.y).contains(&p.y) && (high - low).cross(p - low) > 0.0 {
            inside = !inside;
        }
    }
    inside
}

fn classified_zone(
    point: Vec2,
    polygons: &[(Zone, &[Vec2])],
    contains: impl Fn(Vec2, &[Vec2]) -> bool,
) -> Zone {
    polygons
        .iter()
        .rev()
        .find_map(|(zone, outline)| contains(point, outline).then_some(*zone))
        .unwrap_or(Zone::OutOfBounds)
}

fn zone_rounding_gap(
    polygons: &[(Zone, &[Vec2])],
    point: Vec2,
    recorded: i64,
    computed: i64,
) -> Option<Real> {
    if classified_zone(point, polygons, exact_polygon_contains) as i64 != computed
        || classified_zone(point, polygons, reference_zone_contains) as i64 != recorded
    {
        return None;
    }
    polygons
        .iter()
        .filter(|(zone, _)| [recorded, computed].contains(&(*zone as i64)))
        .filter_map(|(_, outline)| reference_edge_gap(point, outline))
        .min_by(Real::total_cmp)
}

fn matches_crossings(
    polygons: &[(Zone, &[Vec2])],
    from: Vec2,
    to: Vec2,
    from_zone: i32,
    computed: &Dict,
) -> bool {
    let (Some(Variant::PackedFloat64Array(fractions)), Some(Variant::PackedInt32Array(zones))) =
        (computed.get("fractions"), computed.get("zones"))
    else {
        return false;
    };
    let travel = to - from;
    let length = travel.length();
    let mut breaks = Vec::new();
    if length > 0.0 {
        for (_, outline) in polygons {
            for (&a, &b) in outline.iter().zip(outline.iter().cycle().skip(1)) {
                let side_a = travel.cross(a - from);
                let side_b = travel.cross(b - from);
                if side_a == side_b || side_a * side_b > 0.0 {
                    continue;
                }
                let edge_point = a.lerp(b, side_a / (side_a - side_b));
                let t = (edge_point - from).dot(travel) / travel.length_squared();
                if 0.0 < t && t <= 1.0 + 8.0 * Real::EPSILON {
                    breaks.push(t.min(1.0));
                }
            }
        }
    }
    breaks.sort_unstable_by(Real::total_cmp);
    let mut remaining = breaks.as_slice();
    let mut expected = Vec::new();
    let mut zone = from_zone;
    while let Some((&t, rest)) = remaining.split_first() {
        remaining = &rest[rest.partition_point(|u| (u - t) * length < 1e-6)..];
        let stop = remaining.first().copied().unwrap_or(1.0);
        let after = if (stop - t) * length < 1e-6 {
            t + 1e-6 / length
        } else {
            (t + stop) * 0.5
        };
        let entered =
            classified_zone(from.lerp(to, after), polygons, exact_polygon_contains) as i32;
        if zone != entered {
            expected.push((t, entered));
            zone = entered;
        }
    }
    fractions.len() == expected.len()
        && zones.len() == expected.len()
        && expected
            .iter()
            .zip(fractions.iter().zip(zones))
            .all(|(&(t, zone), (&fraction, &kind))| (fraction - t).abs() <= 1e-10 && kind == zone)
}

fn outside_penalties<'a>(point: Vec2, outlines: impl Iterator<Item = &'a [Vec2]>) -> bool {
    outlines.into_iter().all(|outline| {
        !exact_polygon_contains(point, outline)
            && outline
                .iter()
                .zip(outline.iter().cycle().skip(1))
                .all(|(&a, &b)| {
                    let edge = b - a;
                    let offset = point - a;
                    if edge == Vec2::ZERO {
                        return offset != Vec2::ZERO;
                    }
                    !(edge.cross(offset) == 0.0
                        && (0.0..=edge.length_squared()).contains(&offset.dot(edge)))
                })
    })
}

pub(super) struct Queries<'a> {
    pub(super) course: &'a Course,
    obstacles: ObstacleIndex<'a>,
    spans: Vec<[Vec2; 4]>,
    zones: ZoneMap<'a>,
    legal: LegalMap<'a>,
}

impl<'a> Queries<'a> {
    pub(super) fn new(course: &'a Course) -> Self {
        let data = course.data();
        let spans = data
            .surface_tiles
            .iter()
            .chain(data.property.iter().map(|property| &property.surface))
            .map(|tile| {
                let min = tile.frame().origin;
                let max = tile.frame().span_end();
                [min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y)]
            })
            .collect();
        Self {
            course,
            spans,
            obstacles: course.obstacles(),
            zones: course.zone_map(),
            legal: course.legal(),
        }
    }

    fn zone_polygons(&self) -> Vec<(Zone, &[Vec2])> {
        let data = self.course.data();
        self.spans
            .iter()
            .map(|outline| (Zone::InPlay, outline.as_slice()))
            .chain(
                [
                    (LegalKind::PenaltyArea, Zone::PenaltyArea),
                    (LegalKind::OutOfBounds, Zone::OutOfBounds),
                ]
                .into_iter()
                .flat_map(|(kind, zone)| {
                    data.regions
                        .iter()
                        .filter(move |region| region.legal == kind)
                        .map(move |region| (zone, region.polygon.as_slice()))
                }),
            )
            .collect()
    }

    fn rounded_penalty(&self, input: &Dict, expected: &Variant, actual: &Variant) -> bool {
        let point = point3(input.get("point").unwrap());
        let plan = Vec2::new(point.x, point.z);
        let expected_area = if let Some(radius) = input.get("radius_m") {
            if scalar(radius) != 0.0 || !matches!(actual, Variant::Nil) {
                return false;
            }
            expected.as_dict()
        } else {
            let area = |kind| Variant::from(kind as i64);
            if *expected != area(Area::Penalty) || *actual != area(Area::General) {
                return false;
            }
            let contains = |key| exact_polygon_contains(plan, &polygon(input.get(key).unwrap()));
            let bunker = self
                .course
                .data()
                .bunkers
                .iter()
                .any(|bunker| exact_polygon_contains(plan, &bunker.outline));
            if contains("tee") || contains("green") || bunker {
                return false;
            }
            None
        };
        let penalties = || {
            self.course
                .data()
                .regions
                .iter()
                .filter(|r| r.legal == LegalKind::PenaltyArea)
        };
        if !outside_penalties(plan, penalties().map(|region| region.polygon.as_slice())) {
            return false;
        }
        penalties().any(|region| {
            if expected_area.is_some_and(|area| {
                polygon(area.get("outline").unwrap()) != region.polygon
                    || !matches!(area.get("red"), Some(Variant::Bool(red))
                                if *red == (region.penalty_color == PenaltyColor::Red))
            }) {
                return false;
            }
            region
                .polygon
                .iter()
                .zip(region.polygon.iter().cycle().skip(1))
                .any(|(&a, &b)| rounded_edge_reaches(plan, a, b))
        })
    }

    fn rounded_crossing(&self, input: &Dict, expected: &Variant, actual: &Variant) -> bool {
        let recorded = expected.as_dict().unwrap();
        let computed = actual.as_dict().unwrap();
        let (Some(Variant::PackedFloat64Array(fractions)), Some(Variant::PackedInt32Array(zones))) =
            (recorded.get("fractions"), recorded.get("zones"))
        else {
            return false;
        };
        let ([at], [zone]) = (fractions.as_slice(), zones.as_slice()) else {
            return false;
        };
        if !matches!(computed.get("fractions"), Some(Variant::PackedFloat64Array(f)) if f.is_empty())
        {
            return false;
        }
        let from = point3(input.get("from").unwrap());
        let to = point3(input.get("to").unwrap());
        let polygons = self.zone_polygons();
        let plan = |v: Vec3| Vec2::new(v.x, v.z);
        let Some(Variant::Int(from_zone)) = input.get("from_zone") else {
            return false;
        };
        if !matches_crossings(
            &polygons,
            plan(from),
            plan(to),
            from_zone.value() as i32,
            computed,
        ) {
            return false;
        }
        let vertex = plan(from.lerp(to, *at));
        let probe = round_plan(plan(from.lerp(to, (at + 1.0) * 0.5)));
        classified_zone(probe, &polygons, reference_zone_contains) as i32 == *zone
            && polygons
                .iter()
                .any(|(_, outline)| reference_edge_gap(probe, outline).is_some())
            && polygons
                .iter()
                .any(|(_, outline)| outline.contains(&vertex))
    }

    fn rounded_trunk(&self, case: &Dict, hit: &Dict) -> bool {
        if !matches!(hit.get("kind"), Some(Variant::Int(k)) if k.value() == ObstacleKind::Trunk as i64)
        {
            return false;
        }
        let input = case.get("input").unwrap().as_dict().unwrap();
        let from = point3(input.get("from").unwrap());
        let to = point3(input.get("to").unwrap());
        let radius = scalar(input.get("radius").unwrap());
        let fraction = scalar(hit.get("fraction").unwrap());
        let point = point3(hit.get("point").unwrap());
        let plan = |p: Vec3| Vec2::new(p.x, p.z);
        let delta = plan(to) - plan(from);
        let normal = point3(hit.get("normal").unwrap());
        let precision = COURSE_STEPS * Real::from(f32::EPSILON);
        self.course.data().trees.instances.iter().any(|tree| {
            let relative = plan(from - tree.base);
            let reach = tree.trunk_radius_m + radius;
            let Some(rounded) = rounded_trunk_fraction(relative, delta, reach) else {
                return false;
            };
            let at = from.lerp(to, rounded);
            let top = if tree.cone {
                tree.height_m
            } else {
                tree.crown_base_m + 0.5 * (tree.height_m - tree.crown_base_m)
            };
            let radial = (plan(point) - plan(tree.base)).normalized();
            (rounded - fraction).abs() <= precision
                && (tree.base.y - radius..=tree.base.y + top).contains(&at.y)
                && (at - point).length() <= precision * at.length().max(1.0)
                && (radial - plan(normal)).length() <= precision
                && normal.y == 0.0
        })
    }

    fn first_ray_entry(&self, origin: Vec3, direction: Vec3, limit: Real) -> Option<Real> {
        if direction == Vec3::ZERO || limit <= 0.0 || origin.y < self.course.height_at(origin) {
            return None;
        }
        let end = origin + direction * limit;
        let clearance = |distance| {
            let point = origin + direction * distance;
            point.y - self.course.height_at(point)
        };
        let grade = direction.y.abs()
            + self.course.maximum_grade() * Vec2::new(direction.x, direction.z).length();
        let ground = ground_ray_entry(clearance, limit, grade);
        let obstacle = exact_solid_entry(self.course.data(), origin, end, 0.0)
            .map(|(fraction, _)| fraction * limit);
        let crown = self
            .course
            .data()
            .trees
            .instances
            .iter()
            .filter_map(|tree| crown_entry(tree, origin, direction, limit))
            .min_by(Real::total_cmp);
        [ground, obstacle, crown]
            .into_iter()
            .flatten()
            .min_by(Real::total_cmp)
    }

    fn sampled_ray(&self, origin: Vec3, direction: Vec3, max: Real) -> (Vec3, usize) {
        let round = |p: Vec3| {
            let [x, y, z] = [p.x, p.y, p.z].map(|v| Real::from(v as f32));
            Vec3::new(x, y, z)
        };
        let rounded_direction = round(direction);
        let height = |point: Vec3| point.y - self.course.height_at(point);
        let tallest = self
            .course
            .data()
            .trees
            .instances
            .iter()
            .map(|tree| tree.height_m)
            .fold(0.0, Real::max);
        let rate = (-rounded_direction.y
            + self.course.maximum_grade()
                * Vec2::new(rounded_direction.x, rounded_direction.z).length())
        .max(0.05);
        let mut at = origin;
        let mut distance = 0.0;
        let mut steps = 0;
        while distance < max && height(at) >= 0.0 && direction != Vec3::ZERO {
            let step = (height(at) / rate).clamp(
                0.25,
                if height(at) < tallest + 1.0 {
                    1.0
                } else {
                    25.0
                },
            );
            let next = round(at + round(rounded_direction * step));
            if let Some(hit) = self.obstacles.obstacle_hit(at, next, 0.0)
                && height(hit.point) >= 0.0
            {
                return (hit.point, steps + 1);
            }
            let enters_ground = height(next) < 0.0;
            let enters_crown = self.obstacles.canopy_density(next) > 0.0;
            if enters_ground || enters_crown {
                let (mut lo, mut hi) = (0.0, 1.0);
                for _ in 0..14 {
                    let mid = (lo + hi) * 0.5;
                    let point = round(at.lerp(next, mid));
                    if if enters_ground {
                        height(point) < 0.0
                    } else {
                        self.obstacles.canopy_density(point) > 0.0
                    } {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
                return (round(at.lerp(next, hi)), steps + 1);
            }
            at = next;
            distance += step;
            steps += 1;
        }
        (NO_RAY_HIT, steps)
    }

    fn ray_surface(&self, point: Vec3, direction: Vec3) -> RaySurface {
        // The fixture records a point. Identify its surface from the local geometry.
        let probe = direction * 0.01;
        if let Some(hit) = self
            .obstacles
            .obstacle_hit(point - probe, point + probe, 0.0)
        {
            let gap_m = (point - hit.point).dot(hit.normal).abs();
            if gap_m <= RAY_TOLERANCE_M {
                return RaySurface {
                    kind: RaySurfaceKind::Obstacle(hit.kind),
                    normal: hit.normal,
                    gap_m,
                };
            }
        }
        if let Some(crown) = self
            .course
            .data()
            .trees
            .instances
            .iter()
            .filter_map(|tree| crown_surface(tree, point))
            .min_by(|a, b| a.gap_m.total_cmp(&b.gap_m))
        {
            return crown;
        }
        RaySurface {
            kind: RaySurfaceKind::Ground,
            normal: self.course.normal_at(point),
            gap_m: (point.y - self.course.height_at(point)).abs(),
        }
    }

    fn run(&self, case: &Dict) -> Result<Variant, String> {
        let call = case.get("call").unwrap().as_str().unwrap();
        let input = case.get("input").unwrap().as_dict().unwrap();
        let field = |key| input.get(key).unwrap();
        let plan = |key| point2(field(key));
        let point = |key| point3(field(key));
        let number = |key| scalar(field(key));
        let outline = |key| polygon(field(key));
        Ok(match call {
            "ground_at" => vector3(self.course.ground_at(plan("p"))),
            "zone_at" => (self.zones.zone_at(point("position")) as i64).into(),
            "obstacle_hit" => self
                .obstacles
                .obstacle_hit(point("from"), point("to"), number("radius"))
                .map_or(Variant::Nil, |hit| {
                    object([
                        ("kind", (hit.kind as i64).into()),
                        ("point", vector3(hit.point)),
                        ("normal", vector3(hit.normal)),
                        ("fraction", hit.fraction.into()),
                    ])
                }),
            "canopy_density" => self.obstacles.canopy_density(point("position")).into(),
            "zone_map.crossings" => {
                let Variant::Int(zone) = field("from_zone") else {
                    panic!("expected from_zone integer");
                };
                let zone = match zone.value() {
                    0 => Zone::InPlay,
                    1 => Zone::PenaltyArea,
                    2 => Zone::OutOfBounds,
                    zone => return Err(format!("unknown zone {zone}")),
                };
                let mut out = Vec::new();
                self.zones
                    .crossings(point("from"), point("to"), zone, &mut out);
                object([
                    (
                        "fractions",
                        Variant::PackedFloat64Array(out.iter().map(|hit| hit.fraction).collect()),
                    ),
                    (
                        "zones",
                        Variant::PackedInt32Array(out.iter().map(|hit| hit.zone as i32).collect()),
                    ),
                ])
            }
            "legal.penalty_at" => self
                .legal
                .penalty_at(point("point"), number("radius_m"))
                .map_or(Variant::Nil, |area| {
                    object([
                        ("outline", packed_polygon(area.outline)),
                        ("red", Variant::Bool(area.color == PenaltyColor::Red)),
                    ])
                }),
            "legal.bunker_at" => {
                packed_polygon(self.legal.bunker_at(point("point")).unwrap_or(&[]))
            }
            "legal.area_at" => {
                (self
                    .legal
                    .area_at(point("point"), &outline("tee"), &outline("green"))
                    as i64)
                    .into()
            }
            "legal.out_of_bounds" => Variant::Bool(
                self.legal
                    .out_of_bounds(point("centre"), number("radius_m")),
            ),
            "legal.in_lost_ball_zone" => {
                Variant::Bool(self.legal.in_lost_ball_zone(point("centre")))
            }
            "legal.penalty_area.edge_near" => {
                let outline = outline("outline");
                vector2(
                    PenaltyArea {
                        outline: &outline,
                        color: PenaltyColor::Red,
                    }
                    .edge_near(plan("point"))
                    .unwrap(),
                )
            }
            "stance.centre_m" => vector2(
                StanceFootprint {
                    lead_foot_m: plan("lead_foot_m"),
                    trail_foot_m: plan("trail_foot_m"),
                    club_entry_m: plan("club_entry_m"),
                }
                .centre_m(),
            ),
            "stance.to_world" => vector3(StanceFootprint::to_world(
                plan("point_m"),
                point("ball"),
                point("aim"),
            )),
            "ray_hit" => vector3(
                self.obstacles
                    .ray_hit(point("origin"), point("direction"), number("max_m"))
                    .unwrap_or(NO_RAY_HIT),
            ),
            _ => return Err(format!("unknown query {call:?}")),
        })
    }
}

fn ellipse_entry(start: Vec3, travel: Vec3, radius: Real) -> Option<Real> {
    let speed = travel.length();
    if speed <= 1e-6 || start.length_squared() < radius * radius {
        return None;
    }
    let direction = travel / speed;
    let offset = start.cross(direction).length();
    if offset > radius {
        return None;
    }
    let half_chord = fm::sqrt((radius - offset) * (radius + offset));
    let t = (-start.dot(direction) - half_chord) / speed;
    (0.0..=1.0).contains(&t).then_some(t)
}

fn wall_entry(
    a: Vec3,
    b: Vec3,
    from: Vec3,
    to: Vec3,
    width: Real,
    height: Real,
    radius: Real,
) -> Option<Real> {
    let up = Vec3::new(0.0, 1.0, 0.0);
    let horizontal = |p: Vec3| Vec3::new(p.x, 0.0, p.z);
    let edge = horizontal(b - a);
    let length = edge.length();
    if length < 1e-6 {
        return None;
    }
    let tangent = edge / length;
    let across = tangent.cross(up);
    let half = width * 0.5 + radius;
    let axes = [tangent, across, up];
    let limits = [
        [-radius, length + radius],
        [-half, half],
        [
            a.y.min(b.y) - a.y - 2.0,
            a.y.max(b.y) - a.y + height + radius,
        ],
    ];
    let mut near: Real = 0.0;
    let mut far: Real = 1.0;
    for (axis, [lo, hi]) in axes.into_iter().zip(limits) {
        let start = (from - a).dot(axis);
        let travel = (to - from).dot(axis);
        if travel.abs() < 1e-12 {
            if !(lo..=hi).contains(&start) {
                return None;
            }
            continue;
        }
        let a = (lo - start) / travel;
        let b = (hi - start) / travel;
        near = near.max(a.min(b));
        far = far.min(a.max(b));
    }
    (0.0 < near && near <= far).then_some(near)
}

fn face_entry([a, b, c]: [Vec3; 3], from: Vec3, to: Vec3, radius: Real) -> Option<Real> {
    let normal = (b - a).cross(c - a).normalized();
    let side = normal.dot(from - a);
    let travel = normal.dot(to - from);
    if travel.abs() < 1e-12 {
        return None;
    }
    let t = (radius.copysign(side) - side) / travel;
    if !(0.0..=1.0).contains(&t) {
        return None;
    }
    let on_face = from.lerp(to, t) - normal * radius.copysign(side);
    [(a, b), (b, c), (c, a)]
        .into_iter()
        .all(|(p, q)| (q - p).cross(on_face - p).dot(normal) >= -1e-11)
        .then_some(t)
}

fn exact_solid_entry(
    data: &CourseData,
    from: Vec3,
    to: Vec3,
    radius: Real,
) -> Option<(Real, ObstacleKind)> {
    let mut first = None;
    let mut offer = |kind, fraction: Option<Real>| {
        if let Some(t) = fraction
            && first.is_none_or(|(old, _)| t < old)
        {
            first = Some((t, kind));
        }
    };
    let horizontal = |p: Vec3| Vec3::new(p.x, 0.0, p.z);
    for tree in &data.trees.instances {
        let travel = horizontal(to - from);
        let start = horizontal(from - tree.base);
        let top = if tree.cone {
            tree.height_m
        } else {
            (tree.height_m + tree.crown_base_m) * 0.5
        };
        let entry = ellipse_entry(start, travel, tree.trunk_radius_m + radius)
            .filter(|&t| (tree.base.y - radius..=tree.base.y + top).contains(&from.lerp(to, t).y));
        offer(ObstacleKind::Trunk, entry);
    }
    for wall in &data.walls {
        for pair in wall.base.windows(2) {
            offer(
                ObstacleKind::Wall,
                wall_entry(
                    pair[0],
                    pair[1],
                    from,
                    to,
                    wall.width_m,
                    wall.height_m,
                    radius,
                ),
            );
        }
    }
    for rock in &data.rocks {
        let up = Vec3::new(0.0, 1.0, 0.0);
        let radii = rock.radii + Vec3::new(radius, radius, radius);
        let local = |v: Vec3| v.rotated(up, -rock.yaw_rad).cdiv(radii);
        offer(
            ObstacleKind::Rock,
            ellipse_entry(local(from - rock.center), local(to - from), 1.0),
        );
    }
    for face in &data.bunker_faces {
        for (base, top) in face.base.windows(2).zip(face.top.windows(2)) {
            for triangle in [[base[0], base[1], top[1]], [base[0], top[1], top[0]]] {
                offer(
                    ObstacleKind::BunkerFace,
                    face_entry(triangle, from, to, radius),
                );
            }
        }
    }
    first
}

fn matches_solid_entry(
    exact: Option<(Real, ObstacleKind)>,
    from: Vec3,
    to: Vec3,
    actual: &Variant,
) -> bool {
    let Some((first, kind)) = exact else {
        return matches!(actual, Variant::Nil);
    };
    let Some(actual) = actual.as_dict() else {
        return false;
    };
    matches!(actual.get("kind"), Some(Variant::Int(code)) if code.value() == kind as i64)
        && (scalar(actual.get("fraction").unwrap()) - first).abs() <= 1e-9
        && (point3(actual.get("point").unwrap()) - from.lerp(to, first)).length() <= 1e-8
}

#[derive(Clone, Copy)]
struct GroundBracket {
    lo: Real,
    hi: Real,
    above_lo: Real,
    above_hi: Real,
}

fn ground_ray_entry(clearance: impl Fn(Real) -> Real, length: Real, grade: Real) -> Option<Real> {
    let start = clearance(0.0);
    if start < 0.0 || length <= 0.0 {
        return None;
    }
    let mut stack = vec![GroundBracket {
        lo: 0.0,
        hi: length,
        above_lo: start,
        above_hi: clearance(length),
    }];
    while let Some(GroundBracket {
        lo,
        hi,
        above_lo,
        above_hi,
    }) = stack.pop()
    {
        if above_lo <= 0.0 && lo > 0.0 {
            return Some(lo);
        }
        // A Lipschitz bound on height along the ray can rule out both halves at once.
        if above_lo.min(above_hi) > grade * (hi - lo) * 0.5 {
            continue;
        }
        if hi - lo <= RAY_TOLERANCE_M * 0.125 {
            if above_hi < 0.0 {
                return Some(hi);
            }
            continue;
        }
        let middle = (lo + hi) * 0.5;
        let above_middle = clearance(middle);
        stack.push(GroundBracket {
            lo: middle,
            hi,
            above_lo: above_middle,
            above_hi,
        });
        stack.push(GroundBracket {
            lo,
            hi: middle,
            above_lo,
            above_hi: above_middle,
        });
    }
    None
}

fn crown_entry(tree: &Tree, origin: Vec3, direction: Vec3, length: Real) -> Option<Real> {
    let height = tree.height_m - tree.crown_base_m;
    let radius = tree.crown_radius_m;
    if tree.density <= 0.0 || height <= 0.0 || radius <= 0.0 {
        return None;
    }
    let up = Vec3::new(0.0, 1.0, 0.0);
    let start = origin - tree.base - up * tree.crown_base_m;
    if !tree.cone {
        let radii = Vec3::new(radius, height * 0.5, radius);
        let p = (start - up * (height * 0.5)).cdiv(radii);
        return if p.length_squared() <= 1.0 {
            Some(0.0)
        } else {
            ellipse_entry(p, (direction * length).cdiv(radii), 1.0).map(|t| t * length)
        };
    }
    let slope = radius / height;
    let clearance = |p: Vec3| {
        Vec2::new(p.x, p.z).length_squared() - (slope * (height - p.y)) * (slope * (height - p.y))
    };
    let within = |s: Real| {
        let p = start + direction * s;
        (0.0..=height).contains(&p.y) && clearance(p) <= 1e-11
    };
    if within(0.0) {
        return Some(0.0);
    }
    let a = direction.x * direction.x + direction.z * direction.z
        - (slope * direction.y) * (slope * direction.y);
    let half_b = start.x * direction.x
        + start.z * direction.z
        + slope * slope * (height - start.y) * direction.y;
    let c = clearance(start);
    let discriminant = half_b * half_b - a * c;
    let mut candidates = [Real::INFINITY; 3];
    if direction.y != 0.0 {
        candidates[0] = -start.y / direction.y;
    }
    if a.abs() < 1e-14 {
        if half_b != 0.0 {
            candidates[1] = -c / (2.0 * half_b);
        }
    } else if discriminant >= 0.0 {
        let root = fm::sqrt(discriminant);
        candidates[1] = (-half_b - root) / a;
        candidates[2] = (-half_b + root) / a;
    }
    candidates
        .into_iter()
        .filter(|&s| (0.0..=length).contains(&s) && within(s))
        .min_by(Real::total_cmp)
}

fn matches_ray_entry(actual: Real, first: Real) -> bool {
    (actual - first).abs() <= RAY_TOLERANCE_M
}

fn rounded_edge_reaches(point: Vec2, a: Vec2, b: Vec2) -> bool {
    let edge = get_closest_point_to_segment(point, a, b);
    edge != point && round_plan(edge) == point
}

fn rounded_trunk_fraction(relative: Vec2, delta: Vec2, reach: Real) -> Option<Real> {
    let planar_gap = (relative.cross(delta) / delta.length()).abs() - reach;
    if !(planar_gap > 0.0 && planar_gap.is_finite()) {
        return None;
    }
    let offset = round_plan(relative);
    let delta = round_plan(delta);
    let dot = |a: Vec2, b: Vec2| Real::from(a.x as f32 * b.x as f32 + a.y as f32 * b.y as f32);
    let a = dot(delta, delta);
    let b = 2.0 * dot(offset, delta);
    let c = dot(offset, offset) - reach * reach;
    let discriminant = b * b - 4.0 * a * c;
    if discriminant < 0.0 || a < 1e-12 || c < 0.0 {
        return None;
    }
    let fraction = (-b - fm::sqrt(discriminant)) / (2.0 * a);
    (0.0..=1.0).contains(&fraction).then_some(fraction)
}

fn round_plan(point: Vec2) -> Vec2 {
    Vec2::new(Real::from(point.x as f32), Real::from(point.y as f32))
}

fn scalar(value: &Variant) -> Real {
    let Variant::Float(value) = value else {
        panic!("expected Float, got {value:?}");
    };
    value.value()
}

fn point2(value: &Variant) -> Vec2 {
    let Variant::Vector2(vector) = value else {
        panic!("expected Vector2, got {value:?}");
    };
    let [x, y] = vector.values;
    Vec2::new(x, y)
}

fn point3(value: &Variant) -> Vec3 {
    let Variant::Vector3(vector) = value else {
        panic!("expected Vector3, got {value:?}");
    };
    let [x, y, z] = vector.values;
    Vec3::new(x, y, z)
}

fn polygon(value: &Variant) -> Vec<Vec2> {
    let Variant::PackedVector2Array(points) = value else {
        panic!("expected polygon, got {value:?}");
    };
    points
        .values
        .iter()
        .map(|&[x, z]| Vec2::new(x, z))
        .collect()
}

fn vector2(vector: Vec2) -> Variant {
    Variant::Vector2(Components {
        values: [vector.x, vector.y],
        width: FloatWidth::Double,
    })
}

fn vector3(vector: Vec3) -> Variant {
    Variant::Vector3(Components {
        values: [vector.x, vector.y, vector.z],
        width: FloatWidth::Double,
    })
}

fn packed_polygon(outline: &[Vec2]) -> Variant {
    Variant::PackedVector2Array(Components {
        values: outline.iter().map(|p| [p.x, p.y]).collect(),
        width: FloatWidth::Double,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grazing_trunk_contact_amplifies_discriminant_rounding() {
        let squared_error = 1e-8;
        assert_eq!(chord_rounding(0.0, squared_error), 1e-4);
        assert!(chord_rounding(0.01, squared_error) < 1e-6);
    }

    #[test]
    fn rounded_closest_edge_can_reach_a_point_outside_the_penalty_area() {
        let point = Vec2::new(1024.0, 1024.0);
        let edge = |gap| {
            [
                Vec2::new(1023.0, point.y + gap),
                Vec2::new(1025.0, point.y + gap),
            ]
        };
        let [a, b] = edge(0.00001);
        assert!(rounded_edge_reaches(point, a, b));
        let [a, b] = edge(0.001);
        assert!(!rounded_edge_reaches(point, a, b));
        let [a, b] = edge(1e-5);
        let inside = [a, b, Vec2::new(1025.0, 1023.0), Vec2::new(1023.0, 1023.0)];
        let outside = [a, b, Vec2::new(1025.0, 1025.0), Vec2::new(1023.0, 1025.0)];
        assert!(!outside_penalties(point, [inside.as_slice()].into_iter()));
        assert!(outside_penalties(point, [outside.as_slice()].into_iter()));
    }

    #[test]
    fn a_single_precision_trunk_discriminant_can_turn_a_near_miss_into_a_tangent() {
        let delta = Vec2::new(64.0, 0.0);
        assert_eq!(
            rounded_trunk_fraction(Vec2::new(-32.0, 1.00001), delta, 1.0),
            Some(0.5)
        );
        assert_eq!(
            rounded_trunk_fraction(Vec2::new(-32.0, 1.01), delta, 1.0),
            None
        );
    }

    #[test]
    fn a_small_height_residual_cannot_hide_an_early_ray_hit() {
        let clearance = |s: Real| 0.001 - 1e-5 * (s - 100.0);
        let first = ground_ray_entry(clearance, 250.0, 1e-5).unwrap();
        assert!(matches_ray_entry(200.0, first));
        assert!(!matches_ray_entry(100.0, first));
    }

    #[test]
    fn ray_starting_inside_a_crown_hits_near_the_origin() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testdata/courses/test-course");
        let course = Course::open(&path).unwrap();
        let queries = Queries::new(&course);
        let tree = course
            .data()
            .trees
            .instances
            .iter()
            .find(|tree| tree.density > 0.0 && tree.height_m > tree.crown_base_m)
            .unwrap();
        let origin = tree.base + Vec3::new(0.0, (tree.height_m + tree.crown_base_m) * 0.5, 0.0);
        let direction = Vec3::new(0.0, 1.0, 0.0);
        let point = queries.obstacles.ray_hit(origin, direction, 2.0).unwrap();
        let input = Dict::new(vec![
            ("origin".into(), vector3(origin)),
            ("direction".into(), vector3(direction)),
            ("max_m".into(), 2.0.into()),
        ]);
        let mut rays = RayErrors::default();
        rays.check("synthetic", &input, &queries, point, point);
        assert!(rays.report());
    }

    #[test]
    fn a_missing_solid_hit_cannot_use_a_different_trunks_rounded_tangent() {
        let from = Vec3::new(0.0, 1.0, 0.0);
        let to = Vec3::new(10.0, 1.0, 0.0);
        let collision = ellipse_entry(Vec3::new(-5.0, 0.0, 0.0), to - from, 1.0).unwrap();
        let first = Some((collision, ObstacleKind::Trunk));
        assert!(!matches_solid_entry(first, from, to, &Variant::Nil));
        let hit = |fraction| {
            object([
                ("kind", (ObstacleKind::Trunk as i64).into()),
                ("fraction", Variant::from(fraction)),
                ("point", vector3(from.lerp(to, fraction))),
            ])
        };
        assert!(matches_solid_entry(first, from, to, &hit(0.4)));
        assert!(!matches_solid_entry(first, from, to, &hit(0.8)));
    }

    #[test]
    fn a_missing_zone_entry_cannot_use_reference_rounding() {
        let outline = [
            Vec2::new(1.0, -1.0),
            Vec2::new(3.0, -1.0),
            Vec2::new(3.0, 1.0),
            Vec2::new(1.0, 1.0),
        ];
        let polygons = [(Zone::PenaltyArea, outline.as_slice())];
        let check = |fractions, zones| {
            let crossings = object([
                ("fractions", Variant::PackedFloat64Array(fractions)),
                ("zones", Variant::PackedInt32Array(zones)),
            ]);
            matches_crossings(
                &polygons,
                Vec2::ZERO,
                Vec2::new(2.0, 0.0),
                Zone::OutOfBounds as i32,
                crossings.as_dict().unwrap(),
            )
        };
        assert!(check(vec![0.5], vec![Zone::PenaltyArea as i32]));
        assert!(!check(vec![], vec![]));
    }

    #[test]
    fn a_reference_zone_departure_needs_a_nearby_edge() {
        let outline = [
            Vec2::new(100.0, 100.0),
            Vec2::new(200.0, 200.0),
            Vec2::new(100.0, 200.0),
        ];
        let nearby = Vec2::new(150.0, 149.99999);
        let distant = Vec2::new(150.0, 149.0);
        assert!(nearby.y < nearby.x);
        assert!(reference_zone_contains(nearby, &outline));
        assert!(reference_edge_gap(nearby, &outline).is_some());
        assert!(reference_edge_gap(distant, &outline).is_none());
        let polygons = [(Zone::PenaltyArea, outline.as_slice())];
        assert!(
            zone_rounding_gap(
                &polygons,
                nearby,
                Zone::PenaltyArea as i64,
                Zone::OutOfBounds as i64
            )
            .is_some()
        );
        assert!(
            zone_rounding_gap(
                &polygons,
                nearby,
                Zone::PenaltyArea as i64,
                Zone::InPlay as i64
            )
            .is_none()
        );
    }
}
