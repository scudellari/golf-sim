use golf_sim_base::{
    ClubId, Real, Surface, Vec2, Vec3,
    digest::object,
    fm,
    geometry::is_point_in_polygon,
    trace::{NoProbe, Probe},
    variant::{Array, Components, Dict, FloatWidth, Variant},
};
use golf_sim_course::course::{Course, LegalKind, Tree, Wall};
use golf_sim_physics::{
    facade as sim,
    plane::{ConstantAir, PlaneGround},
    samplers::{AirSample, GroundSample, GroundSampler, ZoneCrossing},
    values::{
        BallSpec, ClubCategory, ClubSpec, Contact, Cup, Delivery, Launch, LaunchReadout, Lie,
        ObstacleHit, ObstacleKind, ShotRequest, ShotResult, Zone,
    },
};

const UP: Vec3 = Vec3::new(0.0, 1.0, 0.0);

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

pub fn shot_snapshot(result: &ShotResult) -> Variant {
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

// Keep the sampler used to port these fixtures so its zones and solids remain comparable.
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

struct MownGround<'a>(Box<dyn GroundSampler + 'a>);

impl GroundSampler for MownGround<'_> {
    fn sample_ground(&self, x: Real, z: Real) -> GroundSample {
        golf_sim_stroke::wrappers::MownGround(self.0.as_ref()).sample_ground(x, z)
    }
}

fn ground<'a>(input: &Dict, course: Option<&'a Course>) -> Box<dyn GroundSampler + 'a> {
    match field(input, "kind").as_str().unwrap() {
        "plane" => Box::new(plane(input)),
        "mown_ground" => Box::new(MownGround(ground(dict(field(input, "ground")), course))),
        "practice_ground" => Box::new(PracticeGround {
            plane: plane(dict(field(input, "plane"))),
            centre: vec3(field(input, "pad_centre")),
            gradient: vec2(field(input, "pad_gradient")),
            inner: number(input, "flat_radius_m"),
            outer: number(input, "blend_radius_m"),
        }),
        "course_ground" => {
            let course = course.expect("course resolved for course_ground");
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

fn check_carry(case: &Dict, carry: Real, bound: impl FnOnce() -> Real, drift: bool) -> Variant {
    let Some(Variant::Float(reference)) = case.get("output") else {
        return carry.into();
    };
    let reference = reference.value();
    let error = (carry - reference).abs();
    let scale = carry.abs().max(reference.abs()).max(1.0);
    if error.is_finite() && error > super::impact::EIGHT_SINGLE_PRECISION_STEPS * scale {
        let bound = bound();
        if error <= bound {
            let id = field(case, "id").as_str().unwrap();
            println!(
                "case {id}: carry {error:.3e} m within {bound:.3e} m from accumulated reference height rounding"
            );
            if !drift {
                return reference.into();
            }
        }
    }
    carry.into()
}

fn compare_trajectory(
    case: &Dict,
    result: &ShotResult,
    context: super::trajectory::FlightContext<'_>,
    drift: bool,
) -> Variant {
    let mut output = shot_snapshot(result);
    let Variant::Dict(actual) = &mut output else {
        unreachable!()
    };
    super::trajectory::compare(case, actual, context, drift);
    output
}

pub(in crate::corpus) fn verify_delivery(
    case: &Dict,
    actual: &Variant,
    course: Option<&Course>,
) -> Result<(), String> {
    let input = dict(field(case, "input"));
    let request = request(dict(field(input, "request")));
    let ground = ground(dict(field(input, "ground")), course);
    let launch = |output: &Variant| -> Result<(), String> {
        super::impact::verify_strike_rounding(
            dict(field(dict(field(case, "output")), "launch")),
            dict(field(dict(output), "launch")),
            &request,
            &*ground,
        )
    };
    launch(actual)
}

pub(in crate::corpus) fn replay(
    case: &Dict,
    course: Option<&Course>,
    probe: &mut impl Probe,
    drift: bool,
) -> Result<Variant, String> {
    let call = field(case, "call")
        .as_str()
        .unwrap()
        .strip_prefix("golf_sim.")
        .unwrap();
    let input = dict(field(case, "input"));
    let arg = |key| dict(field(input, key));
    Ok(match call {
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
                "strike" => {
                    let request = request(arg("request"));
                    let mut actual =
                        launch_snapshot(sim::strike(&request, &conditions, &*ground, probe));
                    if let Variant::Dict(output) = &mut actual {
                        super::impact::compare_launch_rounding(
                            case,
                            dict(field(case, "output")),
                            output,
                            &request,
                            &*ground,
                            drift,
                        )?;
                    }
                    actual
                }
                "simulate_shot" => {
                    let request = request(arg("request"));
                    let result = sim::simulate_shot(
                        &request,
                        &conditions,
                        &*ground,
                        bool_field(input, "playback"),
                        probe,
                    );
                    let context = super::trajectory::FlightContext {
                        ball: &request.ball,
                        air: &conditions,
                        ground: &*ground,
                        t0: request.time_s,
                        dt: golf_sim_physics::tunables::FLIGHT_DT,
                    };
                    let mut shot = compare_trajectory(case, &result, context, drift);
                    if let Variant::Dict(actual) = &mut shot {
                        super::impact::compare_shot_launch(
                            dict(field(case, "output")),
                            actual,
                            |expected, launch| {
                                super::impact::compare_launch_rounding(
                                    case, expected, launch, &request, &*ground, drift,
                                )
                            },
                        )?;
                    }
                    shot
                }
                "simulate_launch" => {
                    let ball = ball(arg("ball"));
                    let time = number(input, "time_s");
                    let result = sim::simulate_launch(
                        &launch(arg("launch")),
                        &ball,
                        time,
                        &conditions,
                        &*ground,
                        cup(field(input, "cup")),
                        bool_field(input, "playback"),
                        probe,
                    );
                    let context = super::trajectory::FlightContext {
                        ball: &ball,
                        air: &conditions,
                        ground: &*ground,
                        t0: time,
                        dt: golf_sim_physics::tunables::FLIGHT_DT,
                    };
                    compare_trajectory(case, &result, context, drift)
                }
                "simulate_drop" => shot_snapshot(&sim::simulate_drop(
                    &ball(arg("ball")),
                    vec3(field(input, "release_point")),
                    number(input, "time_s"),
                    &conditions,
                    &*ground,
                    bool_field(input, "playback"),
                    probe,
                )),
                "carry_to_height" => {
                    let launch = launch(arg("launch"));
                    let ball = ball(arg("ball"));
                    let time = number(input, "time_s");
                    let level = number(input, "height_m");
                    let carry =
                        sim::carry_to_height(&launch, &ball, time, &conditions, &*ground, level);
                    check_carry(
                        case,
                        carry,
                        || {
                            super::flight::carry_rounding_bound(
                                &launch,
                                &ball,
                                time,
                                &conditions,
                                &*ground,
                                level,
                                carry,
                            )
                        },
                        drift,
                    )
                }
                _ => panic!("unknown facade call {call}"),
            }
        }
    })
}

pub(in crate::corpus) fn strokes(
    case: &Dict,
    course: Option<&Course>,
    versions: super::models::Versions,
    drift: bool,
) -> Result<crate::tolerance::CaseOutcome, String> {
    use golf_sim_base::variant;
    use golf_sim_physics::integrator::{self, Options};

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
    match field(case, "call").as_str().unwrap() {
        "shot_integrator.carry_to_height" => {
            let time = number(input, "t0");
            let level = number(input, "height_m");
            let carry =
                integrator::carry_to_height(&launch, &ball, time, &conditions, &*ground, level);
            Ok(check_carry(
                case,
                carry,
                || {
                    super::flight::carry_rounding_bound(
                        &launch,
                        &ball,
                        time,
                        &conditions,
                        &*ground,
                        level,
                        carry,
                    )
                },
                drift,
            )
            .into())
        }
        "shot_integrator.run" => {
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
            let output = shot_snapshot(&result);
            versions.run(|probe| {
                let traced =
                    integrator::run(&launch, &ball, &conditions, &*ground, cup, options, probe);
                assert_eq!(
                    variant::encode(&output).unwrap(),
                    variant::encode(&shot_snapshot(&traced)).unwrap(),
                    "probe changed {}",
                    field(case, "id").as_str().unwrap(),
                );
                let context = super::trajectory::FlightContext {
                    ball: &ball,
                    air: &conditions,
                    ground: &*ground,
                    t0: options.time_s,
                    dt: golf_sim_physics::tunables::FLIGHT_DT * options.step_scale,
                };
                Ok(compare_trajectory(case, &result, context, drift))
            })
        }
        call => Err(format!("unknown integrator call {call:?}")),
    }
}
