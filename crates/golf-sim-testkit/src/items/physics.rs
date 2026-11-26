use std::path::{Path, PathBuf};

use golf_sim_base::{
    Real, Surface, Vec2, Vec3,
    digest::object,
    fm,
    geometry::is_point_in_polygon,
    trace::NoProbe,
    variant::{Array, Components, FloatWidth, Variant},
};
use golf_sim_course::course::{Course, LegalKind, Tree, Wall};
use golf_sim_physics::{
    integrator::{self, Options},
    plane::{ConstantAir, PlaneGround},
    samplers::{GroundSample, GroundSampler, ZoneCrossing},
    tunables as t,
    values::{Cup, Launch, LaunchReadout, ObstacleHit, ObstacleKind, ShotResult, Zone},
};

use crate::determinism::{Item, fixture_dir};

const UP: Vec3 = Vec3::new(0.0, 1.0, 0.0);
const FORWARD: Vec3 = Vec3::new(0.0, 0.0, -1.0);

pub fn items() -> Vec<Item> {
    vec![
        Item::new("stroke.flat_drive", flat_drive),
        Item::new("stroke.flagstick_putt", flagstick_putt),
        Item::new("stroke.course_drive", course_drive),
        Item::new("stroke.course_wall", course_wall),
    ]
}

fn testdata() -> PathBuf {
    fixture_dir().join("..")
}

fn open_course() -> std::sync::Arc<Course> {
    Course::open(&testdata().join(Path::new("courses").join("test-course"))).unwrap()
}

fn flat_drive() -> Variant {
    snapshot(&play(
        Vec3::ZERO,
        FORWARD,
        65.0,
        13.0,
        250.0,
        &PlaneGround::default(),
        None,
    ))
}

fn flagstick_putt() -> Variant {
    let green = PlaneGround::new(
        GroundSample {
            surface: Surface::Green,
            ..GroundSample::default()
        },
        Vec3::ZERO,
    );
    let cup = Cup {
        position: Vec3::ZERO,
        flagstick_in: true,
    };
    snapshot(&play(
        Vec3::new(0.0, 0.0, 0.35),
        FORWARD,
        1.0,
        0.0,
        0.0,
        &green,
        Some(cup),
    ))
}

fn course_drive() -> Variant {
    let course = open_course();
    let ground = CourseGround::new(&course, 0.5, 0.3, 0.0);
    let start = Vec3::new(74.0, course.height_at(Vec3::new(74.0, 0.0, 64.0)), 64.0);
    snapshot(&play(
        start,
        Vec3::new(1.0, 0.0, 0.0),
        65.0,
        11.0,
        250.0,
        &ground,
        None,
    ))
}

fn course_wall() -> Variant {
    let course = open_course();
    let ground = CourseGround::new(&course, 0.5, 0.3, 0.0);
    let wall = &course.data().walls[0];
    let edge = wall.base[1] - wall.base[0];
    let aim = Vec3::new(edge.z, 0.0, -edge.x).normalized();
    let start = wall.base[0].lerp(wall.base[1], 0.5) + UP * (0.35 - t::BALL.radius_m) - aim * 2.0;
    snapshot(&play(start, aim, 25.0, 0.0, 0.0, &ground, None))
}

fn play(
    at: Vec3,
    aim: Vec3,
    speed: Real,
    angle_deg: Real,
    spin: Real,
    ground: &dyn GroundSampler,
    cup: Option<Cup>,
) -> ShotResult {
    let launch = Launch::from_readout(
        &t::BALL,
        at,
        aim,
        LaunchReadout {
            ball_speed_mps: speed,
            launch_angle_rad: angle_deg.to_radians(),
            spin_rate_rad_s: spin,
            ..LaunchReadout::default()
        },
    );
    integrator::run(
        &launch,
        &t::BALL,
        &ConstantAir::default(),
        ground,
        cup,
        Options::default(),
        &mut NoProbe,
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use golf_sim_base::{
        corpus,
        trace::Trace,
        variant::{self, Dict},
    };
    use golf_sim_physics::{
        samplers::AirSample,
        values::{BallSpec, Contact},
    };

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
        let air = dict(field(input, "air"));
        let mut conditions = ConstantAir::new(AirSample {
            temperature_c: number(air, "temperature_c"),
            pressure_pa: number(air, "pressure_pa"),
            relative_humidity: number(air, "relative_humidity"),
            mean_wind_mps: vec3(field(air, "mean_wind_mps")),
            gust_mps: vec3(field(air, "gust_mps")),
            wind_reference_height_m: number(air, "wind_reference_height_m"),
            roughness_length_m: number(air, "roughness_length_m"),
        });
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
                let mut slope = Vec3::new(-sample.normal.x, 0.0, -sample.normal.z)
                    / sample.normal.y
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

    fn run_case(case: &Dict, course: &Course) -> Variant {
        assert_eq!(field(case, "call").as_str(), Some("shot_integrator.run"));
        let input = dict(field(case, "input"));
        let launch = launch(dict(field(input, "launch")));
        let b = dict(field(input, "ball"));
        let ball = BallSpec {
            mass_kg: number(b, "mass_kg"),
            radius_m: number(b, "radius_m"),
            inertia_factor: number(b, "inertia_factor"),
        };
        let conditions = conditions(dict(field(input, "conditions")));
        let ground = ground(dict(field(input, "ground")), course);
        let cup = field(input, "cup").as_dict().map(|d| Cup {
            position: vec3(field(d, "position")),
            flagstick_in: bool_field(d, "flagstick_in"),
        });
        let options = Options {
            time_s: number(input, "t0"),
            playback: bool_field(input, "playback"),
            release: bool_field(input, "drop"),
            step_scale: number(input, "step_scale"),
        };
        let result = integrator::run(
            &launch,
            &ball,
            &conditions,
            &*ground,
            cup,
            options,
            &mut NoProbe,
        );
        let output = snapshot(&result);
        let mut trace = Trace::default();
        let traced = integrator::run(
            &launch,
            &ball,
            &conditions,
            &*ground,
            cup,
            options,
            &mut trace,
        );
        assert_eq!(
            variant::encode(&output).unwrap(),
            variant::encode(&snapshot(&traced)).unwrap(),
            "trace changed {}",
            field(case, "id").as_str().unwrap()
        );
        output
    }

    #[test]
    fn reference_strokes() {
        use crate::tolerance;
        use std::{
            collections::{BTreeMap, BTreeSet},
            convert::Infallible,
            fs,
        };

        const NEEDS_DAY: &str = "play/test-course/1/001501";
        let file = testdata()
            .join("physics")
            .join("strokes")
            .join("fixture.bin");
        let mut fixture = corpus::read(&file).unwrap();
        let course = open_course();
        let Variant::Dict(fields) = &mut fixture else {
            unreachable!();
        };
        let Some(Variant::Array(cases)) = fields.get_mut("cases") else {
            unreachable!();
        };
        assert_eq!(cases.items.len(), 234);
        cases.items.retain(|value| {
            let case = dict(value);
            if field(case, "id").as_str() != Some(NEEDS_DAY) {
                return true;
            }
            assert_eq!(
                field(case, "call").as_str(),
                Some("shot_integrator.carry_to_height")
            );
            let conditions = dict(field(dict(field(case, "input")), "conditions"));
            assert_eq!(field(conditions, "kind").as_str(), Some("generated_air"));
            println!("Skipped {NEEDS_DAY}: carry-to-height requires the day generator");
            false
        });
        assert_eq!(cases.items.len(), 233);
        let temp = std::env::temp_dir().join(format!("golf-sim-strokes-{}", std::process::id()));
        fs::create_dir(&temp).unwrap();
        let temp = TempDirectory(temp);
        let testdata_copy = temp.0.join("testdata");
        fs::create_dir(&testdata_copy).unwrap();
        for name in ["tolerances.toml", "departures.toml"] {
            fs::copy(testdata().join(name), testdata_copy.join(name)).unwrap();
        }
        let selected = testdata_copy.join("fixture.bin");
        let body = variant::encode(&fixture).unwrap();
        let mut bytes = b"GSCORP01".to_vec();
        bytes.extend(u32::try_from(body.len()).unwrap().to_le_bytes());
        bytes.extend(body);
        fs::write(&selected, bytes).unwrap();

        let mut outputs = BTreeMap::new();
        let mut discrete = Vec::new();
        let mut max_rest = (0.0, String::new());
        let mut max_sample = (0.0, String::new());
        let fixture = tolerance::Fixture::read(&selected).unwrap();
        let drift = fixture
            .drift("physics", |case| {
                let id = field(case, "id").as_str().unwrap();
                let actual = run_case(case, &course);
                let expected = dict(field(case, "output"));
                let output = dict(&actual);
                for key in ["end_reason", "rest_surface"] {
                    if field(expected, key) != field(output, key) {
                        discrete.push(format!(
                            "{id} {key}: {} -> {}",
                            integer(field(expected, key)),
                            integer(field(output, key))
                        ));
                    }
                }
                for (name, fields) in [
                    ("events", &["kind", "surface", "zone"][..]),
                    ("touchdowns", &["surface"][..]),
                ] {
                    let codes = |d: &Dict| -> Vec<_> {
                        entries(field(d, name))
                            .iter()
                            .map(|row| {
                                fields
                                    .iter()
                                    .map(|field_name| integer(field(dict(row), field_name)))
                                    .collect::<Vec<_>>()
                            })
                            .collect()
                    };
                    let before = codes(expected);
                    let after = codes(output);
                    if before != after {
                        discrete.push(format!("{id} {name}: {before:?} -> {after:?}"));
                    }
                }
                let rest_delta = (vec3(field(expected, "rest_position"))
                    - vec3(field(output, "rest_position")))
                .length();
                if rest_delta > max_rest.0 {
                    max_rest = (rest_delta, id.to_owned());
                }

                let Variant::PackedVector3Array(before) = field(expected, "positions") else {
                    panic!("expected playback positions")
                };
                let Variant::PackedVector3Array(after) = field(output, "positions") else {
                    panic!("expected playback positions")
                };
                for (&[ex, ey, ez], &[ax, ay, az]) in before.values.iter().zip(&after.values) {
                    let distance = Vec3::new(ax - ex, ay - ey, az - ez).length();
                    if distance > max_sample.0 {
                        max_sample = (distance, id.to_owned());
                    }
                }
                assert!(outputs.insert(id.to_owned(), actual.clone()).is_none());
                Ok::<_, Infallible>(actual)
            })
            .unwrap();
        let replayed = outputs.len();
        assert_eq!(replayed + drift.skipped.len(), 233);
        let checked = tolerance::check(&selected, "physics", |case| {
            let id = field(case, "id").as_str().unwrap();
            Ok::<_, Infallible>(outputs.remove(id).unwrap())
        });
        println!("{replayed} strokes produced bit-identical results with Trace and NoProbe");
        println!(
            "{} discrete end, event, touchdown or surface differences",
            discrete.len()
        );
        for difference in &discrete {
            println!("{difference}");
        }
        println!(
            "Largest rest difference {:.9} m ({})",
            max_rest.0, max_rest.1
        );
        println!(
            "Largest indexed playback position difference {:.9} m ({})",
            max_sample.0, max_sample.1
        );
        print!("{drift}");
        println!("Fields beyond the physics/strokes tolerance:");
        for difference in &drift.out_of_bounds {
            println!("{difference}");
        }
        let failed_cases = drift
            .out_of_bounds
            .iter()
            .map(|difference| &difference.case_id)
            .collect::<BTreeSet<_>>();
        println!(
            "{} fields beyond tolerance across {} strokes",
            drift.out_of_bounds.len(),
            failed_cases.len()
        );
        assert!(
            discrete.is_empty() && failed_cases.is_empty(),
            "{} strokes beyond physics/strokes tolerance; first checker result: {checked:?}",
            failed_cases.len(),
        );
        let checked = checked.unwrap();
        assert_eq!(checked.checked, replayed);
        assert_eq!(checked.skipped, drift.skipped);
    }

    struct TempDirectory(PathBuf);

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
