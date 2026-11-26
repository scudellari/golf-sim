use std::{
    convert::Infallible,
    fs,
    path::{Path, PathBuf},
};

use golf_sim_base::{
    ClubId, Real, Surface, Vec2, Vec3, corpus,
    digest::object,
    fm,
    geometry::is_point_in_polygon,
    trace::{NoProbe, Phase, Trace},
    variant::{self, Array, Components, Dict, FloatWidth, Variant},
};
use golf_sim_physics::{
    facade as sim,
    plane::{ConstantAir, PlaneGround},
    samplers::{AirSample, GroundSample, GroundSampler, ZoneCrossing},
    values::{
        BallSpec, ClubCategory, ClubSpec, Contact, Cup, Delivery, EventKind, Launch, LaunchReadout,
        Lie, ObstacleHit, ObstacleKind, ShotRequest, ShotResult, Zone, units,
    },
};
use golf_sim_testkit::tolerance;
use golf_sim_validate::{Conditions, Corpus, Kind, Model, Quantities, Quantity, Status, Target};

// The fixture uses the public synthetic course. The reader is needed only by this integration test.
#[path = "../../golf-sim-course/src/bundle.rs"]
#[allow(dead_code)]
mod bundle;
#[path = "../../golf-sim-course/src/course.rs"]
#[allow(dead_code)]
mod course;
#[path = "../../golf-sim-course/src/grid.rs"]
#[allow(dead_code)]
mod grid;

use course::{Course, LegalKind, Tree, Wall};

const UP: Vec3 = Vec3::new(0.0, 1.0, 0.0);

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

fn vector(point: Vec3) -> Variant {
    Variant::Vector3(Components {
        values: [point.x, point.y, point.z],
        width: FloatWidth::Double,
    })
}

fn array(items: impl IntoIterator<Item = Variant>) -> Variant {
    Variant::Array(Array::new(items.into_iter().collect()))
}

macro_rules! record {
    ($($key:ident: $value:expr),+ $(,)?) => {
        object([$( (stringify!($key), ($value).into()) ),+])
    };
}

fn snapshot(result: &ShotResult) -> Variant {
    record! {
        launch: result.launch.map_or(Variant::Nil, launch_snapshot),
        times: Variant::PackedFloat64Array(result.times.clone()),
        positions: Variant::PackedVector3Array(Components {
            values: result.positions.iter().map(|p| [p.x, p.y, p.z]).collect(),
            width: FloatWidth::Double,
        }),
        events: array(result.events.iter().map(|event| record! {
            kind: event.kind as i64,
            time_s: event.time_s,
            position: vector(event.position),
            velocity: vector(event.velocity),
            spin: vector(event.spin),
            surface: event.surface as i64,
            zone: event.zone as i64,
        })),
        touchdowns: array(result.touchdowns.iter().map(|touchdown| record! {
            time_s: touchdown.time_s,
            position: vector(touchdown.position),
            contact_point: vector(touchdown.contact_point),
            velocity: vector(touchdown.velocity),
            depth_m: touchdown.depth_m,
            surface: touchdown.surface as i64,
        })),
        end_reason: result.end_reason as i64,
        rest_position: vector(result.rest_position),
        rest_surface: result.rest_surface as i64,
        embedded: Variant::Bool(result.embedded),
    }
}

fn launch_snapshot(launch: Launch) -> Variant {
    let readout = launch.readout;
    let absent = Vec3::new(Real::INFINITY, Real::INFINITY, Real::INFINITY);
    record! {
        position: vector(launch.position),
        velocity: vector(launch.velocity),
        spin: vector(launch.spin),
        aim: vector(launch.aim),
        ball_speed_mps: readout.ball_speed_mps,
        launch_angle_rad: readout.launch_angle_rad,
        launch_direction_rad: readout.launch_direction_rad,
        spin_rate_rad_s: readout.spin_rate_rad_s,
        spin_axis_rad: readout.spin_axis_rad,
        club_speed_mps: launch.club_speed_mps,
        club_speed_at_ball_mps: launch.club_speed_at_ball_mps,
        smash_factor: launch.smash_factor,
        club_path_rad: launch.club_path_rad,
        attack_angle_rad: launch.attack_angle_rad,
        face_angle_rad: launch.face_angle_rad,
        dynamic_loft_rad: launch.dynamic_loft_rad,
        face_to_path_rad: launch.face_to_path_rad,
        spin_loft_rad: launch.spin_loft_rad,
        valid: Variant::Bool(launch.valid),
        contact: launch.contact as i64,
        strike_toe_m: launch.strike_toe_m,
        strike_up_m: launch.strike_up_m,
        low_point_ahead_m: launch.low_point_ahead_m,
        low_point_height_m: launch.low_point_height_m,
        turf_entry_behind_m: launch.turf_entry_behind_m,
        turf_contact_start: vector(launch.turf_contact_start.unwrap_or(absent)),
        turf_contact_end: vector(launch.turf_contact_end.unwrap_or(absent)),
        turf_contact_depth_m: launch.turf_contact_depth_m,
        interference: launch.interference,
        ball_temperature_c: launch.ball_temperature_c,
    }
}

// The course queries for solids and zones are still stubs. Keep the adapter in
// the fixture runner until those queries are available to the constant-day world.
struct CourseGround<'a> {
    course: &'a Course,
    firmness: Real,
    moisture: Real,
    green_speed_m: Real,
    zones: Vec<(Zone, Vec<Vec2>)>,
}

impl<'a> CourseGround<'a> {
    fn new(course: &'a Course, firmness: Real, moisture: Real, green_speed_m: Real) -> Self {
        let data = course.data();
        let grids = data
            .surface_tiles
            .iter()
            .chain(data.property.iter().map(|p| &p.surface));
        let mut zones = grids
            .map(|grid| {
                let frame = grid.frame();
                let min = frame.origin;
                let max = frame.span_end();
                (
                    Zone::InPlay,
                    vec![min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y)],
                )
            })
            .collect::<Vec<_>>();
        for (legal, zone) in [
            (LegalKind::PenaltyArea, Zone::PenaltyArea),
            (LegalKind::OutOfBounds, Zone::OutOfBounds),
        ] {
            zones.extend(
                data.regions
                    .iter()
                    .filter(|region| region.legal == legal)
                    .map(|region| (zone, region.polygon.clone())),
            );
        }
        Self {
            course,
            firmness,
            moisture,
            green_speed_m,
            zones,
        }
    }
}

impl GroundSampler for CourseGround<'_> {
    fn sample_ground(&self, x: Real, z: Real) -> GroundSample {
        let point = Vec2::new(x, z);
        let split = self.course.ground_split_at(point);
        let mut sample = GroundSample {
            height_m: split.height(),
            normal: Vec3::new(-split.slope.x, 1.0, -split.slope.y).normalized(),
            surface: Surface::Rough,
            firmness: self.firmness,
            moisture: self.moisture,
            ..GroundSample::default()
        };
        if let Some((_, grid)) = self.course.surface_at(point) {
            let material = &self.course.data().materials[usize::from(grid.material_at(point).0)];
            sample.surface = material.kind;
            if matches!(
                material.kind,
                Surface::FirstCut | Surface::Rough | Surface::DeepRough
            ) {
                sample.grass_height_m = material.grass_height_m * grid.stand_at(point);
            }
            if matches!(material.kind, Surface::Green | Surface::Fringe) {
                sample.green_speed_m = self.green_speed_m;
            }
        }
        sample
    }

    fn maximum_grade(&self) -> Real {
        self.course.maximum_grade()
    }

    fn obstacle_hit(&self, from: Vec3, to: Vec3, radius: Real) -> Option<ObstacleHit> {
        let course = self.course.data();
        course
            .trees
            .instances
            .iter()
            .filter_map(|tree| hit_tree(tree, from, to, radius))
            .chain(course.walls.iter().flat_map(|wall| {
                wall.base.windows(2).filter_map(move |segment| {
                    hit_wall(wall, segment[0], segment[1], from, to, radius)
                })
            }))
            .min_by(|a, b| a.fraction.total_cmp(&b.fraction))
    }

    fn canopy_density(&self, at: Vec3) -> Real {
        self.course
            .data()
            .trees
            .instances
            .iter()
            .filter(|tree| {
                let offset = at - tree.base;
                if !(tree.crown_base_m..=tree.height_m).contains(&offset.y) {
                    return false;
                }
                let height = tree.height_m - tree.crown_base_m;
                let horizontal = horizontal(offset).length() / tree.crown_radius_m;
                if tree.cone {
                    horizontal <= (tree.height_m - offset.y) / height
                } else {
                    let vertical = 2.0 * (offset.y - tree.crown_base_m) / height - 1.0;
                    horizontal * horizontal + vertical * vertical <= 1.0
                }
            })
            .fold(0.0, |density, tree| density.max(tree.density))
    }

    fn has_zones(&self) -> bool {
        true
    }

    fn zone_at(&self, at: Vec3) -> Zone {
        self.zones
            .iter()
            .rev()
            .find(|(_, polygon)| is_point_in_polygon(horizontal(at), polygon))
            .map_or(Zone::OutOfBounds, |(zone, _)| *zone)
    }

    fn crossings(&self, from: Vec3, to: Vec3, from_zone: Zone, out: &mut Vec<ZoneCrossing>) {
        out.clear();
        let start = horizontal(from);
        let delta = horizontal(to) - start;
        let length = delta.length();
        if length == 0.0 {
            return;
        }
        let mut hits = Vec::new();
        for (_, polygon) in &self.zones {
            for (&a, &b) in polygon.iter().zip(polygon.iter().cycle().skip(1)) {
                let edge = b - a;
                let denominator = delta.cross(edge);
                if denominator == 0.0 {
                    continue;
                }
                let offset = a - start;
                let fraction = offset.cross(edge) / denominator;
                let along_edge = offset.cross(delta) / denominator;
                if fraction > 0.0 && fraction <= 1.0 && (-1e-9..=1.0 + 1e-9).contains(&along_edge) {
                    hits.push(fraction);
                }
            }
        }
        hits.sort_unstable_by(Real::total_cmp);
        let merge = 1e-6 / length;
        let mut zone = from_zone;
        let mut remaining = hits.as_slice();
        while let Some(&at) = remaining.first() {
            let next = remaining.partition_point(|&f| f - at < merge);
            remaining = &remaining[next..];
            let distance = (remaining.first().copied().unwrap_or(1.0) - at) * 0.5;
            let point = start.lerp(
                horizontal(to),
                at + if distance * 2.0 < merge {
                    merge
                } else {
                    distance
                },
            );
            let entered = self.zone_at(Vec3::new(point.x, 0.0, point.y));
            if entered != zone {
                out.push(ZoneCrossing {
                    fraction: at,
                    zone: entered,
                });
                zone = entered;
            }
        }
    }
}

fn horizontal(vector: Vec3) -> Vec2 {
    Vec2::new(vector.x, vector.z)
}

fn hit_tree(tree: &Tree, from: Vec3, to: Vec3, radius: Real) -> Option<ObstacleHit> {
    let origin = horizontal(from - tree.base);
    let delta = horizontal(to - from);
    let inflated = tree.trunk_radius_m + radius;
    let a = delta.length_squared();
    let b = origin.dot(delta);
    let c = origin.length_squared() - inflated * inflated;
    let discriminant = b * b - a * c;
    if a < 1e-12 || c < 0.0 || discriminant < 0.0 {
        return None;
    }
    let fraction = (-b - fm::sqrt(discriminant)) / a;
    let point = from.lerp(to, fraction);
    let height = if tree.cone {
        tree.height_m
    } else {
        (tree.height_m + tree.crown_base_m) * 0.5
    };
    if !(0.0..=1.0).contains(&fraction)
        || !(tree.base.y - radius..=tree.base.y + height).contains(&point.y)
    {
        return None;
    }
    let direction = horizontal(point - tree.base).normalized();
    Some(ObstacleHit {
        kind: ObstacleKind::Trunk,
        point,
        normal: Vec3::new(direction.x, 0.0, direction.y),
        fraction,
    })
}

fn hit_wall(
    wall: &Wall,
    a: Vec3,
    b: Vec3,
    from: Vec3,
    to: Vec3,
    radius: Real,
) -> Option<ObstacleHit> {
    let edge = horizontal(b - a);
    let length = edge.length();
    if length < 1e-6 {
        return None;
    }
    let along = Vec3::new(edge.x, 0.0, edge.y) / length;
    let across = UP.cross(-along);
    let side = wall.width_m * 0.5 + radius;
    let mut enter = 0.0;
    let mut leave: Real = 1.0;
    let mut normal = Vec3::ZERO;
    let move_by = to - from;
    for (axis, start, lo, hi) in [
        (along, (from - a).dot(along), -radius, length + radius),
        (across, (from - a).dot(across), -side, side),
        (
            UP,
            from.y,
            a.y.min(b.y) - 2.0,
            a.y.max(b.y) + wall.height_m + radius,
        ),
    ] {
        let speed = move_by.dot(axis);
        if speed.abs() < 1e-12 {
            if !(lo..=hi).contains(&start) {
                return None;
            }
            continue;
        }
        let near = (lo - start) / speed;
        let far = (hi - start) / speed;
        if near.min(far) > enter {
            enter = near.min(far);
            normal = if speed > 0.0 { -axis } else { axis };
        }
        leave = leave.min(near.max(far));
        if enter > leave {
            return None;
        }
    }
    (normal != Vec3::ZERO).then(|| ObstacleHit {
        kind: ObstacleKind::Wall,
        point: from.lerp(to, enter),
        normal,
        fraction: enter,
    })
}

fn dict(value: &Variant) -> &Dict {
    value.as_dict().unwrap()
}
fn field<'a>(input: &'a Dict, name: &str) -> &'a Variant {
    input.get(name).unwrap_or_else(|| panic!("missing {name}"))
}
fn number(input: &Dict, name: &str) -> Real {
    match field(input, name) {
        Variant::Float(value) => value.value(),
        Variant::Int(value) => value.value() as Real,
        _ => panic!("expected a number at {name}"),
    }
}
fn integer(value: &Variant) -> i64 {
    let Variant::Int(value) = value else {
        panic!("expected integer")
    };
    value.value()
}
fn bool_field(input: &Dict, name: &str) -> bool {
    let Variant::Bool(value) = field(input, name) else {
        panic!("expected boolean at {name}")
    };
    *value
}
fn vec2(value: &Variant) -> Vec2 {
    let Variant::Vector2(value) = value else {
        panic!("expected Vector2")
    };
    Vec2::new(value.values[0], value.values[1])
}
fn vec3(value: &Variant) -> Vec3 {
    let Variant::Vector3(value) = value else {
        panic!("expected Vector3")
    };
    let [x, y, z] = value.values;
    Vec3::new(x, y, z)
}
fn entries(value: &Variant) -> &[Variant] {
    let Variant::Array(value) = value else {
        panic!("expected array")
    };
    &value.items
}
fn code(value: &Variant) -> usize {
    usize::try_from(integer(value)).unwrap()
}

fn launch(input: &Dict) -> Launch {
    let n = |name| number(input, name);
    let v = |name| vec3(field(input, name));
    let contact = [
        Contact::Face,
        Contact::LeadingEdge,
        Contact::TopEdge,
        Contact::Sand,
        Contact::SideEdge,
        Contact::Miss,
    ];
    Launch {
        position: v("position"),
        velocity: v("velocity"),
        spin: v("spin"),
        aim: v("aim"),
        readout: LaunchReadout {
            ball_speed_mps: n("ball_speed_mps"),
            launch_angle_rad: n("launch_angle_rad"),
            launch_direction_rad: n("launch_direction_rad"),
            spin_rate_rad_s: n("spin_rate_rad_s"),
            spin_axis_rad: n("spin_axis_rad"),
        },
        club_speed_mps: n("club_speed_mps"),
        club_speed_at_ball_mps: n("club_speed_at_ball_mps"),
        smash_factor: n("smash_factor"),
        club_path_rad: n("club_path_rad"),
        attack_angle_rad: n("attack_angle_rad"),
        face_angle_rad: n("face_angle_rad"),
        dynamic_loft_rad: n("dynamic_loft_rad"),
        face_to_path_rad: n("face_to_path_rad"),
        spin_loft_rad: n("spin_loft_rad"),
        valid: bool_field(input, "valid"),
        contact: contact[code(field(input, "contact"))],
        strike_toe_m: n("strike_toe_m"),
        strike_up_m: n("strike_up_m"),
        low_point_ahead_m: n("low_point_ahead_m"),
        low_point_height_m: n("low_point_height_m"),
        turf_entry_behind_m: n("turf_entry_behind_m"),
        turf_contact_start: Some(v("turf_contact_start")).filter(|point| point.is_finite()),
        turf_contact_end: Some(v("turf_contact_end")).filter(|point| point.is_finite()),
        turf_contact_depth_m: n("turf_contact_depth_m"),
        interference: n("interference"),
        ball_temperature_c: n("ball_temperature_c"),
    }
}

fn ground_sample(input: &Dict) -> GroundSample {
    GroundSample {
        surface: Surface::ALL[code(field(input, "surface"))],
        height_m: number(input, "height_m"),
        normal: vec3(field(input, "normal")),
        firmness: number(input, "firmness"),
        moisture: number(input, "moisture"),
        green_speed_m: number(input, "green_speed_m"),
        grass_height_m: number(input, "grass_height_m"),
        grass_density: number(input, "grass_density"),
    }
}

fn plane(input: &Dict) -> PlaneGround {
    let mut ground = PlaneGround::new(
        ground_sample(dict(field(input, "base"))),
        vec3(field(input, "origin")),
    );
    for patch in entries(field(input, "patches")).iter().map(dict) {
        ground.add_patch(
            vec2(field(patch, "min")),
            vec2(field(patch, "max")),
            ground_sample(dict(field(patch, "sample"))),
        );
    }
    for trunk in entries(field(input, "trunks")).iter().map(dict) {
        ground.add_trunk(
            vec2(field(trunk, "c")),
            number(trunk, "r"),
            number(trunk, "h"),
        );
    }
    for canopy in entries(field(input, "canopies")).iter().map(dict) {
        ground.add_canopy(
            vec3(field(canopy, "c")),
            number(canopy, "r"),
            number(canopy, "d"),
        );
    }
    if let Some(zones) = field(input, "zones").as_dict() {
        let decode_zone = |code| [Zone::InPlay, Zone::PenaltyArea, Zone::OutOfBounds][code];
        ground.set_outside_zone(decode_zone(code(field(zones, "outside"))));
        let Variant::PackedInt32Array(kinds) = field(zones, "kinds") else {
            panic!("expected zone kinds")
        };
        let polygons = entries(field(zones, "polygons"));
        assert_eq!(kinds.len(), polygons.len());
        for (&kind, polygon) in kinds.iter().zip(polygons) {
            let Variant::PackedVector2Array(points) = polygon else {
                panic!("expected polygon")
            };
            let &[min, bottom_right, max, top_left] = points.values.as_slice() else {
                panic!("expected rectangle")
            };
            assert_eq!(bottom_right, [max[0], min[1]]);
            assert_eq!(top_left, [min[0], max[1]]);
            ground.add_zone(
                Vec2::new(min[0], min[1]),
                Vec2::new(max[0], max[1]),
                decode_zone(usize::try_from(kind).unwrap()),
            );
        }
    }
    ground
}

fn conditions(input: &Dict) -> ConstantAir {
    assert_eq!(field(input, "kind").as_str(), Some("constant_air"));
    let mut conditions = ConstantAir::new(air_sample(dict(field(input, "air"))));
    for gust in entries(field(input, "gusts")).iter().map(dict) {
        conditions.add_gust(
            number(gust, "start"),
            number(gust, "end"),
            vec3(field(gust, "gust")),
        );
    }
    conditions
}

struct PracticeGround {
    plane: PlaneGround,
    centre: Vec3,
    gradient: Vec2,
    inner: Real,
    outer: Real,
}

impl GroundSampler for PracticeGround {
    fn sample_ground(&self, x: Real, z: Real) -> GroundSample {
        let mut sample = self.plane.sample_ground(x, z);
        let offset = Vec2::new(x - self.centre.x, z - self.centre.z);
        let distance = offset.length();
        if self.gradient != Vec2::ZERO && distance < self.outer {
            let span = self.outer - self.inner;
            let t = ((distance - self.inner) / span).clamp(0.0, 1.0);
            let weight = 1.0 - t * t * (3.0 - 2.0 * t);
            let rise = self.gradient.dot(offset);
            let mut slope = Vec3::new(-sample.normal.x, 0.0, -sample.normal.z) / sample.normal.y
                + Vec3::new(self.gradient.x, 0.0, self.gradient.y) * weight;
            if distance > self.inner {
                slope += Vec3::new(offset.x, 0.0, offset.y) / distance
                    * (rise * (-6.0 * t * (1.0 - t) / span));
            }
            sample.height_m += rise * weight;
            sample.normal = Vec3::new(-slope.x, 1.0, -slope.z).normalized();
        }
        sample
    }

    fn obstacle_hit(&self, from: Vec3, to: Vec3, radius: Real) -> Option<ObstacleHit> {
        self.plane.obstacle_hit(from, to, radius)
    }

    fn canopy_density(&self, position: Vec3) -> Real {
        self.plane.canopy_density(position)
    }
    fn has_zones(&self) -> bool {
        self.plane.has_zones()
    }
    fn zone_at(&self, at: Vec3) -> Zone {
        self.plane.zone_at(at)
    }
    fn crossings(&self, from: Vec3, to: Vec3, zone: Zone, out: &mut Vec<ZoneCrossing>) {
        self.plane.crossings(from, to, zone, out);
    }
    fn is_planar(&self) -> bool {
        self.gradient == Vec2::ZERO && self.plane.is_planar()
    }
    fn is_uniform_flat(&self) -> bool {
        self.gradient == Vec2::ZERO && self.plane.is_uniform_flat()
    }
    fn maximum_grade(&self) -> Real {
        if self.gradient == Vec2::ZERO {
            return Real::INFINITY;
        }
        let normal = self.plane.sample_ground(0.0, 0.0).normal;
        horizontal(normal).length() / normal.y
            + self.gradient.length() * (1.0 + 1.5 * self.outer / (self.outer - self.inner))
    }
}

fn ground<'a>(input: &Dict, course: &'a Course) -> Box<dyn GroundSampler + 'a> {
    match field(input, "kind").as_str().unwrap() {
        "plane" => Box::new(plane(input)),
        "practice_ground" => Box::new(PracticeGround {
            plane: plane(dict(field(input, "plane"))),
            centre: vec3(field(input, "pad_centre")),
            gradient: vec2(field(input, "pad_gradient")),
            inner: number(input, "flat_radius_m"),
            outer: number(input, "blend_radius_m"),
        }),
        "course_ground" => {
            let identity = dict(field(input, "course"));
            assert_eq!(
                field(identity, "id").as_str(),
                Some(course.data().id.as_str())
            );
            assert_eq!(
                field(identity, "revision").as_str(),
                Some(course.revision())
            );
            Box::new(CourseGround::new(
                course,
                number(input, "firmness"),
                number(input, "moisture"),
                number(input, "green_speed_m"),
            ))
        }
        kind => panic!("unsupported ground sampler {kind}"),
    }
}

macro_rules! read_fields {
    ($record:expr, $type:ident, $($key:ident),+ $(,)?) => {
        $type { $($key: number($record, stringify!($key))),+ }
    };
    ($record:expr, $type:ident { $($name:ident: $other:expr),+ }, $($key:ident),+ $(,)?) => {
        $type { $($name: $other,)+ $($key: number($record, stringify!($key))),+ }
    };
}

fn club(input: &Dict) -> ClubSpec {
    let category = [
        ClubCategory::Driver,
        ClubCategory::Wood,
        ClubCategory::Hybrid,
        ClubCategory::Iron,
        ClubCategory::Wedge,
        ClubCategory::Putter,
    ][code(field(input, "category"))];
    read_fields!(
        input,
        ClubSpec {
            id: field(input, "id").as_str().unwrap().to_owned(),
            category: category
        },
        loft_rad,
        length_m,
        head_mass_kg,
        cor,
        cor_speed_slope_delta,
        cor_falloff_toe,
        cor_falloff_up,
        cg_depth_m,
        cg_up_m,
        cg_toe_m,
        moi_vertical,
        moi_heel_toe,
        moi_forward,
        bulge_radius_m,
        roll_radius_m,
        face_center_height_m,
        face_height_m,
        face_half_width_m,
        face_friction,
        tangential_restitution_up,
        tangential_restitution_toe,
        tangential_restitution_speed_slope,
        arc_radius_m,
        sole_width_m,
        sole_bounce_rad,
    )
}

fn delivery(input: &Dict) -> Delivery {
    read_fields!(
        input,
        Delivery,
        club_speed_mps,
        path_rad,
        attack_rad,
        face_rad,
        dynamic_loft_rad,
        dynamic_lie_rad,
        strike_toe_m,
        sole_height_m,
    )
}

fn ball(input: &Dict) -> BallSpec {
    read_fields!(input, BallSpec, mass_kg, radius_m, inertia_factor)
}

fn lie(input: &Dict) -> Lie {
    read_fields!(
        input,
        Lie {
            surface: Surface::ALL[code(field(input, "surface"))]
        },
        ball_height_m,
        grass_height_m,
        grass_density,
        sand_firmness,
    )
}

fn ball_output(ball: BallSpec) -> Variant {
    record! {
        mass_kg: ball.mass_kg,
        radius_m: ball.radius_m,
        inertia_factor: ball.inertia_factor,
    }
}

fn club_output(club: ClubSpec) -> Variant {
    record! {
        id: club.id.as_str(),
        category: club.category as i64,
        loft_rad: club.loft_rad,
        length_m: club.length_m,
        head_mass_kg: club.head_mass_kg,
        cor: club.cor,
        cor_speed_slope_delta: club.cor_speed_slope_delta,
        cor_falloff_toe: club.cor_falloff_toe,
        cor_falloff_up: club.cor_falloff_up,
        cg_depth_m: club.cg_depth_m,
        cg_up_m: club.cg_up_m,
        cg_toe_m: club.cg_toe_m,
        moi_vertical: club.moi_vertical,
        moi_heel_toe: club.moi_heel_toe,
        moi_forward: club.moi_forward,
        bulge_radius_m: club.bulge_radius_m,
        roll_radius_m: club.roll_radius_m,
        face_center_height_m: club.face_center_height_m,
        face_height_m: club.face_height_m,
        face_half_width_m: club.face_half_width_m,
        face_friction: club.face_friction,
        tangential_restitution_up: club.tangential_restitution_up,
        tangential_restitution_toe: club.tangential_restitution_toe,
        tangential_restitution_speed_slope: club.tangential_restitution_speed_slope,
        arc_radius_m: club.arc_radius_m,
        sole_width_m: club.sole_width_m,
        sole_bounce_rad: club.sole_bounce_rad,
    }
}

fn air_sample(input: &Dict) -> AirSample {
    AirSample {
        temperature_c: number(input, "temperature_c"),
        pressure_pa: number(input, "pressure_pa"),
        relative_humidity: number(input, "relative_humidity"),
        mean_wind_mps: vec3(field(input, "mean_wind_mps")),
        gust_mps: vec3(field(input, "gust_mps")),
        wind_reference_height_m: number(input, "wind_reference_height_m"),
        roughness_length_m: number(input, "roughness_length_m"),
    }
}

fn air_output(air: AirSample) -> Variant {
    record! {
        temperature_c: air.temperature_c,
        pressure_pa: air.pressure_pa,
        relative_humidity: air.relative_humidity,
        mean_wind_mps: vector(air.mean_wind_mps),
        gust_mps: vector(air.gust_mps),
        wind_reference_height_m: air.wind_reference_height_m,
        roughness_length_m: air.roughness_length_m,
    }
}

fn cup(value: &Variant) -> Option<Cup> {
    value.as_dict().map(|input| Cup {
        position: vec3(field(input, "position")),
        flagstick_in: bool_field(input, "flagstick_in"),
    })
}

fn request(input: &Dict) -> ShotRequest {
    let temperature = number(input, "ball_temperature_c");
    ShotRequest {
        club: club(dict(field(input, "club"))),
        ball: ball(dict(field(input, "ball"))),
        delivery: delivery(dict(field(input, "delivery"))),
        lie: lie(dict(field(input, "lie"))),
        position: vec3(field(input, "position")),
        aim: vec3(field(input, "aim")),
        time_s: number(input, "time_s"),
        ball_temperature_c: (!temperature.is_nan()).then_some(temperature),
        cup: cup(field(input, "cup")),
    }
}

fn replay(case: &Dict, course: &Course) -> Variant {
    let call = field(case, "call")
        .as_str()
        .unwrap()
        .strip_prefix("golf_sim.")
        .unwrap();
    let input = dict(field(case, "input"));
    let arg = |key| dict(field(input, key));
    match call {
        "air_density" => sim::air_density(&air_sample(arg("air"))).into(),
        "wind_at_height" => vector(sim::wind_at_height(
            &air_sample(arg("air")),
            number(input, "height_m"),
        )),
        "pressure_at_altitude" => sim::pressure_at_altitude(number(input, "altitude_m")).into(),
        "standard_air" => air_output(sim::standard_air()),
        "standard_ball" => ball_output(sim::standard_ball()),
        "standard_club" => club_output(sim::standard_club(
            ClubId::from_name(field(input, "id").as_str().unwrap()).unwrap(),
        )),
        "standard_club_ids" => Variant::PackedStringArray(
            sim::standard_club_ids()
                .iter()
                .map(|id| id.name().to_owned())
                .collect(),
        ),
        "sole_height_for_strike" => sim::sole_height_for_strike(
            &club(arg("club")),
            &ball(arg("ball")),
            &delivery(arg("delivery")),
            &lie(arg("lie")),
            number(input, "strike_up_m"),
        )
        .into(),
        _ => {
            let conditions = conditions(arg("conditions"));
            let ground = ground(arg("ground"), course);
            match call {
                "strike" => launch_snapshot(sim::strike(
                    &request(arg("request")),
                    &conditions,
                    &*ground,
                    &mut NoProbe,
                )),
                "simulate_shot" => snapshot(&sim::simulate_shot(
                    &request(arg("request")),
                    &conditions,
                    &*ground,
                    bool_field(input, "playback"),
                    &mut NoProbe,
                )),
                "simulate_launch" => snapshot(&sim::simulate_launch(
                    &launch(arg("launch")),
                    &ball(arg("ball")),
                    number(input, "time_s"),
                    &conditions,
                    &*ground,
                    cup(field(input, "cup")),
                    bool_field(input, "playback"),
                    &mut NoProbe,
                )),
                "simulate_drop" => snapshot(&sim::simulate_drop(
                    &ball(arg("ball")),
                    vec3(field(input, "release_point")),
                    number(input, "time_s"),
                    &conditions,
                    &*ground,
                    bool_field(input, "playback"),
                    &mut NoProbe,
                )),
                "carry_to_height" => sim::carry_to_height(
                    &launch(arg("launch")),
                    &ball(arg("ball")),
                    number(input, "time_s"),
                    &conditions,
                    &*ground,
                    number(input, "height_m"),
                )
                .into(),
                _ => panic!("unknown facade call {call}"),
            }
        }
    }
}

const NEEDS_DAY: [&str; 3] = [
    "play/test-course/2/000054",
    "play/test-course/2/011592",
    "play/test-course/2/017232",
];

struct ReplayDirectory(PathBuf);

impl ReplayDirectory {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("golf-sim-facade-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let scratch = Self(dir);
        fs::create_dir(scratch.0.join("testdata")).unwrap();
        for name in ["tolerances.toml", "departures.toml"] {
            fs::copy(testdata().join(name), scratch.0.join("testdata").join(name)).unwrap();
        }
        scratch
    }

    fn fixture(&self, fixture: &Variant) -> PathBuf {
        let path = self.0.join("testdata/fixture.bin");
        let body = variant::encode(fixture).unwrap();
        let mut bytes = b"GSCORP01".to_vec();
        bytes.extend(u32::try_from(body.len()).unwrap().to_le_bytes());
        bytes.extend(body);
        fs::write(&path, bytes).unwrap();
        path
    }
}

impl Drop for ReplayDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn cases_mut(fixture: &mut Variant) -> &mut Vec<Variant> {
    let Variant::Dict(fields) = fixture else {
        unreachable!();
    };
    let Some(Variant::Array(cases)) = fields.get_mut("cases") else {
        unreachable!();
    };
    &mut cases.items
}

fn case_id(case: &Variant) -> &str {
    field(dict(case), "id").as_str().unwrap()
}

#[test]
fn matches_facade_fixture() {
    let course = Course::open(&testdata().join("courses/test-course")).unwrap();
    let mut fixture = corpus::read(&testdata().join("physics/facade/fixture.bin")).unwrap();
    let cases = cases_mut(&mut fixture);
    assert_eq!(cases.len(), 412);
    let indices: std::collections::BTreeMap<_, _> = cases
        .iter()
        .enumerate()
        .map(|(index, case)| (case_id(case).to_owned(), index))
        .collect();
    let mut skipped = Vec::new();
    cases.retain(|case| {
        let id = case_id(case);
        if !NEEDS_DAY.contains(&id) {
            return true;
        }
        let input = dict(field(dict(case), "input"));
        assert_eq!(
            field(dict(field(input, "conditions")), "kind").as_str(),
            Some("generated_air"),
            "{id}"
        );
        assert_eq!(
            field(dict(field(input, "ground")), "kind").as_str(),
            Some("generated_ground"),
            "{id}"
        );
        skipped.push(id.to_owned());
        println!("Skipped {id} (requires the day generator)");
        false
    });
    assert_eq!(skipped, NEEDS_DAY);
    let golden = |case: &Variant| {
        case_id(case).contains("/test_golden_stroke_is_unchanged/")
            && field(dict(case), "call").as_str() == Some("golf_sim.simulate_shot")
    };
    assert_eq!(cases.iter().filter(|case| golden(case)).count(), 1);

    let scratch = ReplayDirectory::new();
    let run = |case: &Dict| Ok::<_, Infallible>(replay(case, &course));
    let path = scratch.fixture(&fixture);
    let report = tolerance::check(&path, "physics", run).unwrap_or_else(|first| {
        let cases = std::mem::take(cases_mut(&mut fixture));
        let mut failures = Vec::new();
        let total = cases.len();
        for case in cases {
            let index = indices[case_id(&case)];
            let is_golden = golden(&case);
            *cases_mut(&mut fixture) = vec![case];
            match tolerance::check(&scratch.fixture(&fixture), "physics", run) {
                Err(failure) => failures.push(failure.to_string().replacen(
                    "cases[0]",
                    &format!("cases[{index}]"),
                    1,
                )),
                Ok(report) if is_golden && report.checked == 1 => println!("Golden stroke passed"),
                Ok(_) => {}
            }
        }
        println!(
            "{} of {total} replayable facade cases passed",
            total - failures.len()
        );
        panic!("Facade tolerance failed ({first})\n{}", failures.join("\n"));
    });
    assert_eq!(report.checked + report.skipped.len(), 409);
    print!("{report}");
}

fn target_number(target: &Target, name: &str) -> Option<Real> {
    target.inputs.get(name).map(|value| {
        value
            .as_number()
            .unwrap_or_else(|| panic!("{} expected a number for {name}", target.id))
    })
}

fn validation_delivery(target: &Target, loft_deg: Real) -> Delivery {
    let input = |key| target_number(target, key);
    Delivery {
        club_speed_mps: units::mph(input("club_mph").unwrap()),
        attack_rad: input("attack_deg").unwrap().to_radians(),
        dynamic_loft_rad: loft_deg.to_radians(),
        path_rad: input("path_deg").unwrap_or_default().to_radians(),
        face_rad: input("face_deg").unwrap_or_default().to_radians(),
        ..Delivery::default()
    }
}

fn delivery_request(club_id: ClubId, mut delivery: Delivery) -> ShotRequest {
    let club = sim::standard_club(club_id);
    let ball = sim::standard_ball();
    let lie = if club.category == ClubCategory::Driver {
        let lie = Lie::tee(0.035);
        delivery.sole_height_m = sim::sole_height_for_strike(&club, &ball, &delivery, &lie, 0.0);
        lie
    } else {
        Lie {
            ball_height_m: 0.005,
            ..Lie::default()
        }
    };
    ShotRequest::new(club, ball, delivery, lie)
}

fn delivery_loft(target: &Target, club: ClubId, air: &ConstantAir, ground: &PlaneGround) -> Real {
    if let Some(loft) = target_number(target, "dynamic_loft_deg") {
        return loft;
    }
    let launch_deg = target_number(target, "launch_deg").unwrap();
    let (mut low, mut high) = (0.0, 50.0);
    for _ in 0..30 {
        let mid = (low + high) * 0.5;
        let mut delivery = validation_delivery(target, mid);
        delivery.path_rad = 0.0;
        delivery.face_rad = 0.0;
        let request = delivery_request(club, delivery);
        let launch = sim::strike(&request, air, ground, &mut NoProbe);
        if launch.readout.launch_angle_rad.to_degrees() < launch_deg {
            low = mid;
        } else {
            high = mid;
        }
    }
    (low + high) * 0.5
}

fn covered(target: &Target, name: &str) -> bool {
    target.kind == Kind::Delivery
        && if target.inputs.get("club").and_then(Quantity::as_str) == Some("putter") {
            matches!(name, "launch_deg" | "speed_ratio" | "true_roll_in")
        } else {
            matches!(name, "carry_yd" | "curve_yd")
        }
}

struct FacadeModel;

impl Model for FacadeModel {
    fn predict(&self, target: &Target, _: &Conditions) -> Option<Quantities> {
        if !target.measured.keys().any(|name| covered(target, name)) {
            return None;
        }
        let club = ClubId::from_name(target.inputs["club"].as_str().unwrap()).unwrap();
        let air = ConstantAir::new(AirSample {
            temperature_c: 25.0,
            ..sim::standard_air()
        });
        let number = |key: &str, value| (key.to_owned(), Quantity::Number(value));
        if club == ClubId::Putter {
            let loft = target_number(target, "dynamic_loft_deg").unwrap();
            let delivery = validation_delivery(target, loft);
            let lie = Lie {
                surface: Surface::Green,
                ..Lie::default()
            };
            let request = ShotRequest::new(
                sim::standard_club(club),
                sim::standard_ball(),
                delivery,
                lie,
            );
            let green = PlaneGround::new(
                GroundSample {
                    surface: Surface::Green,
                    green_speed_m: 3.048,
                    ..GroundSample::default()
                },
                Vec3::ZERO,
            );
            let stroke = sim::simulate_shot(&request, &air, &green, true, &mut NoProbe);
            let readout = stroke.launch.unwrap().readout;
            let roll = stroke.first_event(EventKind::PureRoll).unwrap();
            let true_roll_m = Vec2::new(roll.position.x, roll.position.z).length();
            return Some(Quantities::from([
                number("launch_deg", readout.launch_angle_rad.to_degrees()),
                number(
                    "speed_ratio",
                    readout.ball_speed_mps / delivery.club_speed_mps,
                ),
                number("true_roll_in", units::to_feet(true_roll_m) * 12.0),
            ]));
        }

        let ground = PlaneGround::default();
        let loft = delivery_loft(target, club, &air, &ground);
        let request = delivery_request(club, validation_delivery(target, loft));
        let stroke = sim::simulate_shot(&request, &air, &ground, true, &mut NoProbe);
        let carry_m = stroke.carry_m();
        let carry_yd = units::to_yards(carry_m);
        let mut predictions = Quantities::from([number("carry_yd", carry_yd)]);
        if target.measured.contains_key("curve_yd") {
            let direction = stroke.launch.unwrap().readout.launch_direction_rad;
            let curve_yd = units::to_yards(stroke.carry_side_m() - carry_m * fm::tan(direction));
            let curve_yd = if target.id.starts_with("X3.") {
                // The source expresses the bend at its published carry.
                let source_carry_yd = match club {
                    ClubId::Driver => 275.0,
                    ClubId::Iron6 => 183.0,
                    _ => panic!("no published carry for {club}"),
                };
                curve_yd * source_carry_yd / carry_yd.max(1.0)
            } else {
                curve_yd
            };
            predictions.extend([number("curve_yd", curve_yd)]);
        }
        Some(predictions)
    }
}

#[test]
fn measured_facade_targets() {
    let corpus = Corpus::load(testdata().join("validation")).unwrap();
    let targets: std::collections::BTreeMap<_, _> = corpus
        .files()
        .iter()
        .flat_map(|file| &file.targets)
        .map(|target| (target.id.as_str(), target))
        .collect();
    let mut report = corpus.run(&FacadeModel);
    for result in &mut report.targets {
        result
            .quantities
            .retain(|name, _| covered(targets[result.id.as_str()], name));
        result.status = result
            .quantities
            .values()
            .map(|result| result.status)
            .max()
            .unwrap_or(Status::NotRun);
    }
    report
        .targets
        .retain(|result| !result.quantities.is_empty());
    report
        .skipped_datasets
        .retain(|entry| entry.dataset.contains("tiers"));
    print!("{}", report.scorecard());
    let mut failed = Vec::new();
    for target in &report.targets {
        for (name, result) in &target.quantities {
            if (result.source_tolerance.is_some() || result.earlier_prediction.is_some())
                && result.status != Status::Passed
            {
                failed.push(format!(
                    "{} {name} predicted {:?}, measured {}, residual {:?}, allowed earlier residual {:?}",
                    target.id, result.predicted, result.measured, result.residual, result.allowed_earlier_residual
                ));
            }
        }
        if target.datasets.contains_key("local-tiers") {
            let carry = &target.quantities["carry_yd"];
            println!(
                "{} carry_yd {} (measured {}, {})",
                target.id,
                carry.predicted.as_ref().unwrap(),
                carry.measured,
                carry.status
            );
        }
    }
    assert!(!report.targets.is_empty());
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    println!("{} measured facade targets passed", report.targets.len());
}

#[test]
fn facade_traces_a_delivery_through_rest_without_changing_the_stroke() {
    let request = delivery_request(
        ClubId::Driver,
        Delivery {
            club_speed_mps: 50.0,
            dynamic_loft_rad: 13.0_f64.to_radians(),
            ..Delivery::default()
        },
    );
    let air = ConstantAir::default();
    let ground = PlaneGround::default();
    let plain = sim::simulate_shot(&request, &air, &ground, true, &mut NoProbe);
    let mut trace = Trace::default();
    let traced = sim::simulate_shot(&request, &air, &ground, true, &mut trace);
    assert_eq!(
        variant::encode(&snapshot(&plain)).unwrap(),
        variant::encode(&snapshot(&traced)).unwrap(),
    );
    for phase in [
        Phase::Delivery,
        Phase::Impact,
        Phase::Launch,
        Phase::Flight,
        Phase::Bounce,
        Phase::Roll,
    ] {
        assert!(
            trace.phases().iter().any(|record| record.phase == phase),
            "no {phase:?}"
        );
    }
    let carry = sim::carry_to_height(
        &plain.launch.unwrap(),
        &request.ball,
        request.time_s,
        &air,
        &ground,
        0.0,
    );
    assert!((carry - plain.carry_m()).abs() < 1e-12);
}
