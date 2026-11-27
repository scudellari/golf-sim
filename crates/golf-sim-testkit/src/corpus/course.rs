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
    course: &Course,
    case: &Dict,
    checks: &mut QueryChecks,
    drift: bool,
) -> Result<Variant, String> {
    let queries = Queries {
        course,
        obstacles: course.obstacles(),
        zones: course.zone_map(),
        legal: course.legal(),
    };
    let mut actual = queries.run(case)?;
    let expected = case.get("output").unwrap();
    let id = case.get("id").unwrap().as_str().unwrap();
    let call = case.get("call").and_then(Variant::as_str);
    let raw = (drift && matches!(call, Some("obstacle_hit" | "ray_hit"))).then(|| actual.clone());
    match call {
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
                    checks.obstacle.vector(
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
                .check(id, input, &queries, expected_point, actual_point);
            if expected_point.is_finite() && actual_point.is_finite() {
                // The geometric checks own the point comparison for finite ray hits.
                actual = expected.clone();
            }
        }
        _ => {}
    }
    Ok(raw.unwrap_or(actual))
}

#[derive(Default)]
pub(super) struct QueryChecks {
    obstacle: QueryErrors,
    ray: RayErrors,
}

impl QueryChecks {
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
