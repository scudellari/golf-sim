use std::path::{Path, PathBuf};

use golf_sim_base::{
    Real, Vec2, Vec3,
    digest::object,
    fm,
    variant::{Components, Dict, FloatWidth, Variant},
};
use golf_sim_course::{
    course::{Course, PenaltyColor, Tree},
    legal::{LegalMap, PenaltyArea, StanceFootprint},
    obstacles::ObstacleIndex,
    zones::ZoneMap,
};
use golf_sim_physics::{
    samplers::ZoneCrossing,
    values::{ObstacleKind, Zone},
};
use golf_sim_testkit::tolerance;

const COURSE_STEPS: Real = 8.0;
const RAY_TOLERANCE_M: Real = 0.002;
const NO_RAY_HIT: Vec3 = Vec3::new(Real::INFINITY, Real::INFINITY, Real::INFINITY);

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

#[test]
fn course_queries_match_reference() {
    let course = Course::open(&testdata().join("courses/test-course")).unwrap();
    let queries = Queries {
        course: &course,
        obstacles: course.obstacles(),
        zones: course.zone_map(),
        legal: course.legal(),
    };
    let path = testdata().join("course/queries/fixture.bin");
    let mut obstacle = QueryErrors::default();
    let mut ray = RayErrors::default();
    let report = tolerance::check(&path, "course", |case| {
        let mut actual = queries.run(case)?;
        let expected = case.get("output").unwrap();
        let id = case.get("id").unwrap().as_str().unwrap();
        match case.get("call").and_then(Variant::as_str) {
            Some("obstacle_hit") => {
                if let (Variant::Dict(expected), Variant::Dict(actual)) = (expected, &mut actual) {
                    let input = case.get("input").unwrap().as_dict().unwrap();
                    let endpoints = ["from", "to"].map(|key| point3(input.get(key).unwrap()));
                    let scale = endpoints
                        .into_iter()
                        .flat_map(|p| [p.x, p.y, p.z])
                        .map(Real::abs)
                        .fold(1.0, Real::max);
                    // The hit point loses precision at course scale before its offset from the
                    // obstacle axis is divided by the radius to compute the normal.
                    let step = scale * Real::from(f32::EPSILON);
                    for field in ["point", "normal"] {
                        obstacle.vector(
                            id,
                            field,
                            expected.get(field).unwrap(),
                            actual.get_mut(field).unwrap(),
                            step,
                            COURSE_STEPS,
                        );
                    }
                    if let (Variant::Float(expected), Variant::Float(actual)) = (
                        expected.get("fraction").unwrap(),
                        actual.get_mut("fraction").unwrap(),
                    ) {
                        obstacle.record(
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
                ray.check(id, input, &queries, expected_point, actual_point);
                if expected_point.is_finite() && actual_point.is_finite() {
                    // The geometric checks own the point comparison for finite ray hits.
                    actual = expected.clone();
                }
            }
            _ => {}
        }
        Ok::<_, String>(actual)
    });
    obstacle.print("obstacle_hit", "single-precision steps at endpoint scale");
    let rays_pass = ray.report();
    let report = report.unwrap();
    assert_eq!(report.checked, 13_172);
    assert!(report.discrete.is_empty());
    assert!(report.skipped.is_empty());
    print!("{report}");
    println!("Zone, legal area and obstacle classifications matched exactly.");
    assert!(
        obstacle.outside.is_empty() && rays_pass,
        "query outputs exceeded tolerance"
    );
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
        if expected == NO_RAY_HIT && actual == NO_RAY_HIT {
            self.misses += 1;
            return;
        }
        if !expected.is_finite() || !actual.is_finite() {
            self.kinds.push(format!(
                "{id} hit presence differs: {expected:?} vs {actual:?}"
            ));
            return;
        }
        self.hits += 1;
        let origin = point3(input.get("origin").unwrap());
        let direction = point3(input.get("direction").unwrap()).normalized();
        let expected_along = (expected - origin).dot(direction);
        let actual_along = (actual - origin).dot(direction);
        // A 25 m march step refined 14 times resolves 1.53 mm, with additional drift
        // from the reference's single-precision positions.
        for (errors, point, along) in [
            (&mut self.reference_ray, expected, expected_along),
            (&mut self.core_ray, actual, actual_along),
        ] {
            let off_ray = (point - origin - direction * along.max(0.0)).length();
            errors.record(id, "point", off_ray * 1000.0, RAY_TOLERANCE_M * 1000.0);
        }

        let expected_surface = queries.ray_surface(expected, direction);
        let surface = queries.ray_surface(actual, direction);
        if expected_surface.kind != surface.kind || expected_surface.gap_m > RAY_TOLERANCE_M {
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

        // Compare the along-ray displacement through its component normal to the surface.
        // This is equivalent to dividing the bound by the sine, including at grazing incidence.
        let sine = direction.dot(surface.normal).abs();
        let height_slack = COURSE_STEPS * Real::from(f32::EPSILON) * actual.y.abs().max(1.0);
        let along_error = (expected_along - actual_along).abs();
        let normal_error = along_error * sine;
        self.along_ray.record(
            id,
            &format!(
                "point (along-ray error {:.6} mm, incidence sine {sine:.8})",
                along_error * 1000.0,
            ),
            normal_error / (RAY_TOLERANCE_M + height_slack),
            1.0,
        );
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

#[test]
fn legal_boundary_counts_touching_as_in_bounds() {
    let course = Course::open(&testdata().join("courses/test-course")).unwrap();
    let legal = course.legal();
    let obstruction = Vec3::new(189.875, 25.0, 256.0);
    assert!(legal.out_of_bounds(obstruction, 0.124));
    assert!(!legal.out_of_bounds(obstruction, 0.125));
    assert!(!legal.out_of_bounds(Vec3::new(190.0, 25.0, 256.0), 0.0));

    let beyond_property = Vec3::new(516.125, 25.0, 256.0);
    assert!(legal.out_of_bounds(beyond_property, 0.124));
    assert!(!legal.out_of_bounds(beyond_property, 0.125));

    let beside_penalty = Vec3::new(496.125, 25.0, 130.0);
    assert!(legal.penalty_at(beside_penalty, 0.124).is_none());
    assert!(legal.penalty_at(beside_penalty, 0.125).is_some());
}

#[test]
fn crossings_belong_to_the_segment_that_ends_at_the_boundary() {
    let course = Course::open(&testdata().join("courses/test-course")).unwrap();
    let map = course.zone_map();
    let start = Vec3::new(471.0, 0.0, 130.0);
    let edge = Vec3::new(472.0, 0.0, 130.0);
    let end = Vec3::new(497.0, 0.0, 130.0);
    let mut hits = Vec::new();
    map.crossings(start, edge, map.zone_at(start), &mut hits);
    assert_eq!(
        hits,
        [ZoneCrossing {
            fraction: 1.0,
            zone: Zone::PenaltyArea
        }]
    );
    let entered = hits[0].zone;
    map.crossings(edge, end, entered, &mut hits);
    assert_eq!(
        hits,
        [ZoneCrossing {
            fraction: 24.0 / 25.0,
            zone: Zone::InPlay
        }]
    );
    map.crossings(end, end, Zone::InPlay, &mut hits);
    assert!(hits.is_empty());
}

#[test]
fn vertical_ray_hits_ground_and_refuses_origins_below_it() {
    let course = Course::open(&testdata().join("courses/test-course")).unwrap();
    let ground = course.height_at(Vec3::ZERO);
    let origin = Vec3::new(0.0, ground + 2.0, 0.0);
    let direction = Vec3::new(0.0, -3.0, 0.0);
    let hit = course.ray_hit(origin, direction, 5.0).unwrap();
    assert_eq!((hit.x, hit.z), (0.0, 0.0));
    assert!((hit.y - ground).abs() < 1e-4);
    assert!(course.ray_hit(origin, Vec3::ZERO, 5.0).is_none());
    let below_ground = Vec3::new(0.0, ground - 0.5, 0.0);
    assert!(course.ray_hit(below_ground, direction, 5.0).is_none());
}

struct Queries<'a> {
    course: &'a Course,
    obstacles: ObstacleIndex<'a>,
    zones: ZoneMap<'a>,
    legal: LegalMap<'a>,
}

impl Queries<'_> {
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
