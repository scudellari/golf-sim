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
    use super::testdata;
    use crate::corpus::FixtureRun;

    #[test]
    fn reference_strokes() {
        let path = testdata().join("physics/strokes/fixture.bin");
        let report = FixtureRun::read(&path, Some("physics"), &[])
            .unwrap()
            .check()
            .unwrap();
        assert!(report.checked > 0);
        assert!(report.skipped.len() >= 16);
        assert!(report.discrete.is_empty(), "{report}");
        print!("{report}");
    }
}
