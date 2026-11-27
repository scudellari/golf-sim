use golf_sim_conditions::grounds::CourseGround;
use golf_sim_course::course::Course;
use std::cell::Cell;

use golf_sim_base::{
    Real, Surface, Vec2, Vec3, fm,
    trace::NoProbe,
    variant::{Components, Dict, FloatWidth, Variant},
};
use golf_sim_physics::{
    impact,
    plane::PlaneGround,
    samplers::{AirSample, GroundSample, GroundSampler},
    strike::{self, PreImpact},
    tunables as t,
    values::{BallSpec, ClubCategory, ClubSpec, Contact, Delivery, Launch, Lie, ShotRequest},
};

pub const EIGHT_SINGLE_PRECISION_STEPS: Real = 8.0 / (1 << 23) as Real;

fn dict(value: &Variant) -> &Dict {
    value.as_dict().expect("expected dictionary")
}

fn field<'a>(record: &'a Dict, key: &str) -> &'a Variant {
    record.get(key).unwrap_or_else(|| panic!("missing {key}"))
}

fn array(value: &Variant) -> &[Variant] {
    let Variant::Array(value) = value else {
        panic!("expected array");
    };
    &value.items
}

fn integer(record: &Dict, key: &str) -> usize {
    let Variant::Int(value) = field(record, key) else {
        panic!("expected integer at {key}");
    };
    value.value().try_into().unwrap()
}

fn number(record: &Dict, key: &str) -> Real {
    match field(record, key) {
        Variant::Float(value) => value.value(),
        Variant::Int(value) => value.value() as Real,
        _ => panic!("expected number at {key}"),
    }
}

fn vec2(record: &Dict, key: &str) -> Vec2 {
    let Variant::Vector2(v) = field(record, key) else {
        panic!("expected Vector2 at {key}");
    };
    let [x, y] = v.values;
    Vec2::new(x, y)
}

fn vec3(record: &Dict, key: &str) -> Vec3 {
    let Variant::Vector3(v) = field(record, key) else {
        panic!("expected Vector3 at {key}");
    };
    let [x, y, z] = v.values;
    Vec3::new(x, y, z)
}

fn vector(v: Vec3) -> Variant {
    Variant::Vector3(Components {
        values: [v.x, v.y, v.z],
        width: FloatWidth::Double,
    })
}

fn turf_point(point: Option<Vec3>) -> Variant {
    vector(point.unwrap_or(Vec3::new(Real::INFINITY, Real::INFINITY, Real::INFINITY)))
}

macro_rules! record {
    ($($key:ident: $value:expr),+ $(,)?) => {
        Variant::Dict(Dict::new(vec![$((stringify!($key).into(), ($value).into())),+]))
    };
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
    ][integer(input, "category")];
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
            surface: Surface::ALL[integer(input, "surface")]
        },
        ball_height_m,
        grass_height_m,
        grass_density,
        sand_firmness,
    )
}

fn ground_sample(input: &Dict) -> GroundSample {
    read_fields!(
        input,
        GroundSample {
            surface: Surface::ALL[integer(input, "surface")],
            normal: vec3(input, "normal")
        },
        height_m,
        firmness,
        moisture,
        green_speed_m,
        grass_height_m,
        grass_density,
    )
}

struct DivotGround {
    depth: Real,
    back: Real,
}

impl GroundSampler for DivotGround {
    fn sample_ground(&self, x: Real, z: Real) -> GroundSample {
        // The reference test sampler takes a Vector3 even when called with scalar x and z.
        let x = Real::from(x as f32);
        let z = Real::from(z as f32);
        let inside = x.abs() < 0.03 && z >= -0.05 && z <= self.back;
        GroundSample {
            height_m: if inside { -self.depth } else { 0.0 },
            ..GroundSample::default()
        }
    }
}

struct GroundReads<'a> {
    reads: &'a [Variant],
    index: Cell<usize>,
}

impl GroundSampler for GroundReads<'_> {
    fn sample_ground(&self, x: Real, z: Real) -> GroundSample {
        let i = self.index.get();
        let read = dict(&self.reads[i]);
        self.index.set(i + 1);
        for (axis, value) in [("x", x), ("z", z)] {
            let expected = number(read, axis);
            assert!(
                (value - expected).abs() <= expected.abs().max(1.0) * 8.0 / 8_388_608.0,
                "ground read {i} {axis}: {value} vs {expected}",
            );
        }
        ground_sample(dict(field(read, "sample")))
    }
}

pub(in crate::corpus) fn unsupported_ground(input: &Dict) -> Option<String> {
    let ground = input.get("ground")?.as_dict()?;
    let kind = ground.get("kind")?.as_str()?;
    match kind {
        "plane" | "course_ground" => None,
        "test_sampler" if is_divot_ground(ground) => None,
        _ => Some(format!("unsupported ground sampler {kind:?}")),
    }
}

fn is_divot_ground(ground: &Dict) -> bool {
    ground
        .get("script")
        .and_then(Variant::as_str)
        .and_then(|script| script.rsplit("::").next())
        == Some("DivotGround")
}

fn with_ground<T>(
    input: &Dict,
    course: Option<&Course>,
    run: impl FnOnce(&dyn GroundSampler) -> T,
) -> Result<T, String> {
    let ground = dict(field(input, "ground"));
    Ok(match field(ground, "kind").as_str().unwrap() {
        "plane" => {
            let mut plane = PlaneGround::new(
                ground_sample(dict(field(ground, "base"))),
                vec3(ground, "origin"),
            );
            for patch in array(field(ground, "patches")).iter().map(dict) {
                plane.add_patch(
                    vec2(patch, "min"),
                    vec2(patch, "max"),
                    ground_sample(dict(field(patch, "sample"))),
                );
            }
            run(&plane)
        }
        "course_ground" => {
            if let Some(reads) = input.get("ground_reads") {
                let ground = GroundReads {
                    reads: array(reads),
                    index: Cell::new(0),
                };
                let output = run(&ground);
                assert_eq!(ground.index.get(), ground.reads.len());
                output
            } else {
                run(&CourseGround::new(
                    course.ok_or("course ground has neither a course identity nor ground reads")?,
                    number(ground, "firmness"),
                    number(ground, "moisture"),
                    number(ground, "green_speed_m"),
                ))
            }
        }
        "test_sampler" if is_divot_ground(ground) => {
            let params = dict(field(ground, "parameters"));
            run(&read_fields!(params, DivotGround, depth, back))
        }
        kind => return Err(format!("unknown ground kind {kind}")),
    })
}

pub fn launch_output(launch: Launch) -> Variant {
    record! {
        position: vector(launch.position),
        velocity: vector(launch.velocity),
        spin: vector(launch.spin),
        aim: vector(launch.aim),
        ball_speed_mps: launch.readout.ball_speed_mps,
        launch_angle_rad: launch.readout.launch_angle_rad,
        launch_direction_rad: launch.readout.launch_direction_rad,
        spin_rate_rad_s: launch.readout.spin_rate_rad_s,
        spin_axis_rad: launch.readout.spin_axis_rad,
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
        turf_contact_start: turf_point(launch.turf_contact_start),
        turf_contact_end: turf_point(launch.turf_contact_end),
        turf_contact_depth_m: launch.turf_contact_depth_m,
        interference: launch.interference,
        ball_temperature_c: launch.ball_temperature_c,
    }
}

fn round_height(world_y: Real, local_y: Real) -> Real {
    let base = world_y as f32;
    Real::from(base + local_y as f32) - Real::from(base)
}

#[derive(Default)]
struct StrikeBounds {
    normal: Real,
    face_up: Real,
    velocity: Real,
    spin: [Real; 3],
}

#[derive(Clone, Copy)]
struct CollisionFrame<'a> {
    request: &'a ShotRequest,
    pre: PreImpact,
    head: impact::Head,
    approach: Vec3,
    temperature: Real,
}

#[derive(Default)]
struct CollisionBounds {
    error: StrikeBounds,
    velocity: Vec3,
    spin: Vec3,
}

fn strike_bounds(
    request: &ShotRequest,
    ground: &(impl GroundSampler + ?Sized),
    launch: &Dict,
) -> StrikeBounds {
    const UP: Vec3 = Vec3::new(0.0, 1.0, 0.0);
    let aim = Vec3::new(request.aim.x, 0.0, request.aim.z).normalized();
    let right = aim.cross(UP);
    let delivery = &request.delivery;
    let pre = strike::before_ball(request, aim, right, ground);
    let frame = CollisionFrame {
        request,
        pre,
        head: impact::head_orientation(&request.club, delivery, aim, right, pre.face_close_rad),
        approach: (aim * fm::cos(delivery.path_rad) + right * fm::sin(delivery.path_rad))
            * fm::cos(delivery.attack_rad)
            + UP * fm::sin(delivery.attack_rad),
        temperature: number(launch, "ball_temperature_c"),
    };
    let contact = integer(launch, "contact");
    let up = number(launch, "strike_up_m");
    let toe = number(launch, "strike_toe_m");
    let height = request.ball.radius_m + request.lie.ball_height_m;
    if pre.sand_fraction * pre.sand_blend > 0.0 {
        return frame
            .bunker_bounds(launch, height, toe, up, contact)
            .unwrap_or_default();
    }
    collision(
        frame,
        height,
        toe,
        up,
        contact,
        Some(vec3(launch, "velocity")),
    )
    .error
}

fn launch_readouts_match(launch: &Dict, request: &ShotRequest) -> bool {
    const UP: Vec3 = Vec3::new(0.0, 1.0, 0.0);
    let velocity = vec3(launch, "velocity");
    let spin = vec3(launch, "spin");
    let forward = Vec3::new(request.aim.x, 0.0, request.aim.z).normalized();
    let right = forward.cross(UP);
    let horizontal = Vec2::new(velocity.dot(forward), velocity.dot(right));
    let side = (forward * horizontal.x + right * horizontal.y)
        .normalized()
        .cross(UP);
    let horizontal_spin = Vec2::new(spin.x, spin.z).length();
    let signed_spin = if spin.dot(side) < 0.0 {
        -horizontal_spin
    } else {
        horizontal_spin
    };
    let speed = velocity.length();
    let club_speed = request.delivery.club_speed_mps;
    let smash = if club_speed > 0.0 {
        speed / club_speed
    } else {
        0.0
    };
    // The core's snapshot preserves the doubles used to compute these readouts.
    [
        ("ball_speed_mps", speed),
        (
            "launch_angle_rad",
            fm::atan2(velocity.y, horizontal.length()),
        ),
        (
            "launch_direction_rad",
            fm::atan2(horizontal.y, horizontal.x),
        ),
        ("spin_rate_rad_s", spin.length()),
        ("spin_axis_rad", fm::atan2(-spin.y, signed_spin)),
        ("smash_factor", smash),
    ]
    .into_iter()
    .all(|(key, derived)| number(launch, key) == derived)
}

impl CollisionFrame<'_> {
    fn face_height(self, center_y: Real, sole_y: Real, toe: Real) -> Option<Real> {
        let club = &self.request.club;
        if toe.abs() > club.face_half_width_m {
            return None;
        }
        let roll = if club.roll_radius_m > 0.0 {
            1.0 / club.roll_radius_m
        } else {
            0.0
        };
        let bulge = if club.bulge_radius_m > 0.0 {
            1.0 / club.bulge_radius_m
        } else {
            0.0
        };
        let bottom = -club.face_center_height_m;
        let top = club.face_height_m + bottom;
        let head = self.head;
        let derivative = head.face_up.y.abs()
            - roll
                * (bottom.abs().max(top.abs()) * head.normal.y.abs() + self.request.ball.radius_m);
        if derivative <= 0.0 {
            return None;
        }
        let sag = |u: Real, x: Real| 0.5 * (u * u * roll + x * x * bulge);
        let leading_y = head.face_up.y * bottom - head.normal.y * sag(bottom, 0.0);
        let residual = |u| {
            let normal =
                (head.normal + head.face_up * (u * roll) + head.toe * (toe * bulge)).normalized();
            center_y
                - normal.y * self.request.ball.radius_m
                - (head.face_up.y * u + head.toe.y * toe - head.normal.y * sag(u, toe))
                + leading_y
                - sole_y
        };
        let (mut lo, mut hi) = (bottom, top);
        let sign = residual(lo).signum();
        if residual(hi).signum() == sign {
            return None;
        }
        for _ in 0..48 {
            let mid = 0.5 * (lo + hi);
            if residual(mid).signum() == sign {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        Some(0.5 * (lo + hi))
    }

    fn bunker_bounds(
        self,
        launch: &Dict,
        center_y: Real,
        toe: Real,
        up: Real,
        contact: usize,
    ) -> Option<StrikeBounds> {
        const UP: Vec3 = Vec3::new(0.0, 1.0, 0.0);
        let pre = self.pre;
        let face = Contact::Face as usize;
        if contact != face || self.request.club.category == ClubCategory::Driver {
            return None;
        }
        let direct_up = self.face_height(center_y, pre.sole_height_m, toe)?;
        if (direct_up - up).abs() > 1e-12 {
            return None;
        }
        let direct = collision(self, center_y, toe, direct_up, face, None);

        let supported_y = self.request.ball.radius_m + self.request.lie.ball_height_m.max(0.0);
        let sole_y = pre.sole_height_m.max(0.0);
        let supported_up = self.face_height(supported_y, sole_y, toe)?;
        let supported_frame = Self {
            pre: PreImpact {
                club_speed_mps: pre.club_speed_mps,
                ..PreImpact::default()
            },
            ..self
        };
        let supported = collision(supported_frame, supported_y, toe, supported_up, face, None);

        let speed = supported.velocity.length();
        let speed_error = supported.error.velocity;
        if speed <= speed_error || speed_error == 0.0 || direct.error.velocity == 0.0 {
            return None;
        }
        let step = Real::from(f32::EPSILON);
        let weight = t::SAND_FACE_WEIGHT;
        let direction = supported
            .velocity
            .normalized()
            .lerp(self.head.normal, weight)
            + UP * (fm::sin(pre.sand_bounce_rad) * t::SAND_BOUNCE_LAUNCH_GAIN);
        let direction_error =
            (1.0 - weight) * 2.0 * speed_error / speed + 8.0 * step * (1.0 + direction.length());
        if direction.length() <= direction_error {
            return None;
        }
        let keep_speed = 1.0 - t::SAND_OPEN_SPEED_LOSS * pre.sand_opening;
        let keep_spin = t::SAND_SPIN_KEEP * (1.0 - t::SAND_OPEN_SPIN_LOSS * pre.sand_opening);
        let splash_velocity = direction.normalized() * (speed * keep_speed);
        let splash_error =
            keep_speed.abs() * (speed_error + 2.0 * speed * direction_error / direction.length());
        let fraction = pre.sand_fraction;
        let blend = pre.sand_blend;
        let access = golf_sim_base::num::lerpf(pre.direct_sand_access, 1.0, fraction);
        let transfer = pre.sand_transfer * access;
        let sand_velocity = supported.velocity.lerp(splash_velocity, fraction) * transfer;
        let sand_spin = supported.spin.lerp(supported.spin * keep_spin, fraction)
            * transfer
            * pre.sand_spin_transfer;
        let velocity = direct.velocity.lerp(sand_velocity, blend);
        let spin = direct.spin.lerp(sand_spin, blend);
        if (vec3(launch, "velocity") - velocity).length() > 1e-10 * velocity.length().max(1.0)
            || (vec3(launch, "spin") - spin).length() > 1e-10 * spin.length().max(1.0)
            || !launch_readouts_match(launch, self.request)
        {
            return None;
        }

        // The two contact branches have different errors and different spin and speed weights.
        let mix_error = |direct_error, supported_error, splash_error, transfer| {
            (1.0 - blend) * direct_error
                + blend * transfer * ((1.0 - fraction) * supported_error + fraction * splash_error)
        };
        let velocity = mix_error(direct.error.velocity, speed_error, splash_error, transfer)
            + 8.0 * step * (direct.velocity.length() + sand_velocity.length());
        let sand_spin_components = [sand_spin.x, sand_spin.y, sand_spin.z];
        let direct_spin_components = [direct.spin.x, direct.spin.y, direct.spin.z];
        let spin = std::array::from_fn(|i| {
            let clean = supported.error.spin[i];
            mix_error(
                direct.error.spin[i],
                clean,
                clean * keep_spin.abs(),
                transfer * pre.sand_spin_transfer,
            ) + 8.0 * step * (direct_spin_components[i].abs() + sand_spin_components[i].abs())
        });
        Some(StrikeBounds {
            velocity,
            spin,
            ..direct.error
        })
    }
}

fn collision(
    frame: CollisionFrame<'_>,
    center_y: Real,
    actual_toe: Real,
    up: Real,
    contact: usize,
    reported_velocity: Option<Vec3>,
) -> CollisionBounds {
    use golf_sim_base::Basis;

    const UP: Vec3 = Vec3::new(0.0, 1.0, 0.0);

    let CollisionFrame {
        request,
        pre,
        head,
        approach,
        temperature,
    } = frame;
    let club = &request.club;
    let ball = &request.ball;
    let delivery = &request.delivery;
    let step = Real::from(f32::EPSILON);
    let toe = delivery.strike_toe_m;
    if ![
        Contact::Face,
        Contact::LeadingEdge,
        Contact::TopEdge,
        Contact::SideEdge,
    ]
    .into_iter()
    .any(|kind| contact == kind as usize)
    {
        return CollisionBounds::default();
    }
    let curvature = |radius| if radius > 0.0 { 1.0 / radius } else { 0.0 };
    let roll = curvature(club.roll_radius_m);
    let bulge = curvature(club.bulge_radius_m);
    let face_normal = |height| {
        (head.normal + head.face_up * (height * roll) + head.toe * (toe * bulge)).normalized()
    };
    let offset = |toe, up| {
        head.face_up * up + head.toe * toe
            - head.normal * (0.5 * (up * up * roll + toe * toe * bulge))
    };
    let origin_y = request.position.y;
    let local_scale =
        center_y.abs() + ball.radius_m + up.abs() + toe.abs() + club.face_center_height_m;
    let dy = (round_height(origin_y, center_y) - center_y).abs() + 8.0 * step * local_scale;

    let (normal, dn, dup, edge_distance, offset_error) = if contact == Contact::Face as usize {
        let face_limit = club
            .face_center_height_m
            .max((club.face_height_m - club.face_center_height_m).abs());
        let derivative =
            head.face_up.y.abs() - roll * (face_limit * head.normal.y.abs() + ball.radius_m);
        if derivative <= 0.0 {
            return CollisionBounds::default();
        }
        let dup = dy / derivative;
        if up - dup < -club.face_center_height_m
            || up + dup > club.face_height_m - club.face_center_height_m
        {
            return CollisionBounds::default();
        }
        (
            face_normal(up),
            roll * dup + EIGHT_SINGLE_PRECISION_STEPS,
            dup,
            0.0,
            dup * (1.0 + roll * (up.abs() + 0.5 * dup)),
        )
    } else {
        // Bound the extended-face tangency before finding the first sphere/edge root.
        let sole = |h| {
            center_y - face_normal(h).y * ball.radius_m - offset(toe, h).y
                + offset(0.0, -club.face_center_height_m).y
        };
        let mut tangent_up = 0.0;
        for _ in 0..6 {
            let derivative = (sole(tangent_up + 0.0001) - sole(tangent_up - 0.0001)) / 0.0002;
            if derivative.abs() <= 1e-5 {
                return CollisionBounds::default();
            }
            tangent_up += (pre.sole_height_m - sole(tangent_up)) / derivative;
        }
        let derivative = head.face_up.y.abs()
            - roll * ((tangent_up.abs() + ball.radius_m) * head.normal.y.abs() + ball.radius_m);
        if derivative <= 0.0 {
            return CollisionBounds::default();
        }
        let dup = (dy + (pre.sole_height_m - sole(tangent_up)).abs()) / derivative;
        if dup > ball.radius_m {
            return CollisionBounds::default();
        }
        let to_edge = face_normal(tangent_up) * ball.radius_m + offset(toe, tangent_up)
            - offset(actual_toe, up);
        let perpendicular = to_edge.slide(approach);
        let chord2 = ball.radius_m * ball.radius_m - perpendicular.length_squared();
        let world_error = 1.5 * step * (origin_y.abs() + 2.0 * local_scale + tangent_up.abs());
        let dperp = world_error + dup * (1.0 + roll * (ball.radius_m + tangent_up.abs() + dup));
        let dc = dperp * (2.0 * perpendicular.length() + dperp);
        if chord2 <= dc {
            return CollisionBounds::default();
        }
        let chord = fm::sqrt(chord2);
        let droot = dc / (chord + fm::sqrt(chord2 - dc));
        let normal = (perpendicular + approach * chord).normalized();
        let dn = (dperp + droot + world_error) / ball.radius_m + EIGHT_SINGLE_PRECISION_STEPS;
        (
            normal,
            dn,
            dup,
            Vec2::new(toe - actual_toe, tangent_up - up).length(),
            0.0,
        )
    };
    let face_offset = offset(actual_toe, up);
    let rc = face_offset + head.forward * club.cg_depth_m
        - head.up * club.cg_up_m
        - head.toe * club.cg_toe_m;

    // The contact point, face center and three CG additions are written into vectors.
    // The point cancels in the arm subtraction, so only the latter four stores enter.
    let world_scale = origin_y.abs() + local_scale + rc.length();
    let drc = 2.0 * step * world_scale + offset_error + 8.0 * step * rc.length();
    let rb = -normal * ball.radius_m;
    let drb = ball.radius_m * dn;
    let ib = ball.moment_of_inertia();
    let inertia = |v: Vec3| {
        head.forward * (head.forward.dot(v) / club.moi_forward)
            + head.up * (head.up.dot(v) / club.moi_vertical)
            + head.toe * (head.toe.dot(v) / club.moi_heel_toe)
    };
    let inv_ic = 1.0
        / club
            .moi_forward
            .min(club.moi_vertical)
            .min(club.moi_heel_toe);
    let k_min = 1.0 / ball.mass_kg + 1.0 / club.head_mass_kg;
    let k_norm = k_min + rb.length_squared() / ib + rc.length_squared() * inv_ic;
    let dk = (2.0 * rb.length() + drb) * drb / ib
        + (2.0 * rc.length() + drc) * drc * inv_ic
        + 16.0 * step * k_norm;
    let response = |v| v * k_min - rb.cross(rb.cross(v)) / ib - rc.cross(inertia(rc.cross(v)));
    let [x, y, z] = [Vec3::new(1.0, 0.0, 0.0), UP, Vec3::new(0.0, 0.0, 1.0)].map(response);
    let k = Basis {
        rows: [
            Vec3::new(x.x, y.x, z.x),
            Vec3::new(x.y, y.y, z.y),
            Vec3::new(x.z, y.z, z.z),
        ],
    };
    let incoming = -approach * (pre.club_speed_mps * (1.0 + toe / club.arc_radius_m));
    let speed = incoming.length();
    let un = incoming.dot(normal);
    let dun = speed * (dn + 4.0 * step);
    let slip = incoming - normal * un;
    let dslip = dun + (un.abs() + dun) * dn + 4.0 * step * speed;
    let toe_axis = head.toe.slide(normal).normalized();
    let up_axis = normal.cross(toe_axis);
    let dtoe = 2.0 * dn / head.toe.slide(normal).length();
    let dup_axis = dn + dtoe;
    let cor_slope = if matches!(club.category, ClubCategory::Driver | ClubCategory::Putter) {
        0.0
    } else {
        t::COR_SPEED_SLOPE + club.cor_speed_slope_delta
    };
    let e = if cor_slope == 0.0 {
        club.cor
    } else {
        (club.cor + cor_slope * (t::COR_REFERENCE_SPEED + un)).min(t::COR_LOW_SPEED_MAX)
    };
    let contact_cor =
        1.0 - club.cor_falloff_toe * actual_toe * actual_toe - club.cor_falloff_up * up * up;
    let edge_cor = 1.0
        - (1.0 - t::EDGE_COR_FACTOR)
            * golf_sim_base::num::smoothstep(0.0, t::EDGE_COR_BLEND_M, edge_distance);
    let restitution = ((e * contact_cor
        + t::COR_PER_DEGREE_C * (temperature - t::COR_TEMPERATURE_REFERENCE_C))
        * edge_cor
        * (1.0 - pre.cushion * t::INTERFERENCE_COR_LOSS))
        .clamp(0.05, 0.95);
    let de = (contact_cor.abs() + 1.0) * cor_slope.abs() * dun
        + if contact == Contact::Face as usize {
            e.abs() * club.cor_falloff_up.abs() * (2.0 * up.abs() + dup) * dup
        } else {
            1.5 * (1.0 - t::EDGE_COR_FACTOR) * dup / t::EDGE_COR_BLEND_M
        };
    let keep = 1.0 - pre.interference * t::INTERFERENCE_TANGENTIAL_LOSS;
    let kt = club.tangential_restitution_toe * keep;
    let ku = (club.tangential_restitution_up
        + club.tangential_restitution_speed_slope
            * (t::COR_REFERENCE_SPEED + un).clamp(0.0, t::TANGENTIAL_SPEED_RANGE_MPS))
        * keep;
    let dku = club.tangential_restitution_speed_slope.abs() * dun * keep.abs();
    let target = normal * (-restitution * un)
        - toe_axis * (kt * slip.dot(toe_axis))
        - up_axis * (ku * slip.dot(up_axis));
    let unconstrained = k.inverse().xform(target - incoming);
    let impulse_error = (restitution * dun
        + (un.abs() + dun) * de
        + restitution * un.abs() * dn
        + kt.abs() * (2.0 * slip.length() * dtoe + dslip)
        + ku.abs() * (2.0 * slip.length() * dup_axis + dslip)
        + dku * (slip.length() + dslip)
        + dk * unconstrained.length()
        + 8.0 * step * speed)
        / k_min;
    let mu = club.face_friction
        * (1.0 - pre.interference * t::INTERFERENCE_FRICTION_LOSS)
        * fm::exp(-t::GRASS_FILM_FRICTION_DECAY * pre.grass_film);
    let jn = unconstrained.dot(normal);
    let tangent = unconstrained - normal * jn;
    let dj_n = impulse_error + unconstrained.length() * dn;
    let cone_error = impulse_error + (1.0 + mu.abs()) * dj_n + jn.abs() * dn;
    let cone_margin = mu * jn - tangent.length();
    let (impulse, dv) = if jn > dj_n && cone_margin > cone_error {
        (unconstrained, impulse_error / ball.mass_kg)
    } else if cone_margin < -cone_error && slip.length() > dslip {
        let dir = normal - slip.normalized() * mu;
        let ddir = dn + mu.abs() * 2.0 * dslip / slip.length();
        let denominator = normal.dot(k.xform(dir));
        let db = dk * (dir.length() + ddir) + k_norm * (ddir + dn * dir.length());
        let a = -(1.0 + restitution) * un;
        let da = (1.0 + restitution) * dun + (un.abs() + dun) * de;
        if denominator <= db {
            return CollisionBounds::default();
        }
        let quotient = a / denominator;
        let dq = (da + quotient.abs() * db) / (denominator - db);
        (
            dir * quotient,
            (dir.length() * dq + (quotient.abs() + dq) * ddir) / ball.mass_kg,
        )
    } else {
        return CollisionBounds {
            error: StrikeBounds {
                normal: dn,
                face_up: dup,
                ..StrikeBounds::default()
            },
            ..CollisionBounds::default()
        };
    };
    let computed_velocity = impulse / ball.mass_kg * pre.direct_sand_access * pre.grass_transfer;
    let computed_spin = rb.cross(impulse) / ib * pre.direct_sand_access * pre.grass_transfer;
    let velocity = reported_velocity.unwrap_or(computed_velocity);
    let factor = (pre.direct_sand_access * pre.grass_transfer).abs();
    let mut dv = dv * factor + 8.0 * step * velocity.length();
    if reported_velocity.is_some() && club.category == ClubCategory::Driver {
        // Clamping the velocity to the smash-limit ball is a nonexpansive projection.
        dv += 4.0 * step * velocity.length();
    }
    // Spin is r_ball × (m * velocity) / inertia, including the same final scaling as velocity.
    let spin = [normal.y, normal.z, normal.x]
        .into_iter()
        .zip([normal.z, normal.x, normal.y])
        .zip([
            (velocity.y, velocity.z),
            (velocity.z, velocity.x),
            (velocity.x, velocity.y),
        ])
        .map(|((a, b), (v, w))| {
            let component = (a.abs() + b.abs()) * dv
                + dn * (v.abs() + w.abs() + 2.0 * dv)
                + 8.0 * step * ((a * w).abs() + (b * v).abs());
            component / (ball.radius_m * ball.inertia_factor)
        })
        .collect::<Vec<_>>();
    CollisionBounds {
        error: StrikeBounds {
            normal: dn,
            face_up: dup,
            velocity: dv,
            spin: [spin[0], spin[1], spin[2]],
        },
        velocity: computed_velocity,
        spin: computed_spin,
    }
}

fn check_strike_fields(expected: &Dict, actual: &Dict) -> Result<(), String> {
    if expected.get("contact") != actual.get("contact") {
        return Err("contact kind differs".into());
    }
    for key in [
        "velocity",
        "spin",
        "ball_speed_mps",
        "launch_angle_rad",
        "launch_direction_rad",
        "spin_rate_rad_s",
        "spin_axis_rad",
        "smash_factor",
        "face_angle_rad",
        "dynamic_loft_rad",
        "face_to_path_rad",
        "spin_loft_rad",
        "strike_toe_m",
        "strike_up_m",
    ] {
        let Some(value) = actual.get(key) else {
            continue;
        };
        let finite = match value {
            Variant::Vector3(v) => v.values.iter().all(|n| n.is_finite()),
            Variant::Float(f) => f.value().is_finite(),
            _ => false,
        };
        if !finite {
            return Err(format!("core {key} is not finite"));
        }
    }
    Ok(())
}

fn exceeds_strike_row(a: Real, b: Real, scale: Real) -> bool {
    (a - b).abs()
        > 1e-6_f64
            .max(1e-5 * a.abs().max(b.abs()))
            .max(EIGHT_SINGLE_PRECISION_STEPS * scale)
}

pub(in crate::corpus) fn compare_launch_rounding(
    case: &Dict,
    expected: &Dict,
    actual: &mut Dict,
    request: &ShotRequest,
    ground: &(impl GroundSampler + ?Sized),
    drift: bool,
) -> Result<(), String> {
    check_strike_fields(expected, actual)?;
    let id = field(case, "id").as_str().unwrap();
    let needs_bound = actual.entries().iter().any(|(key, value)| {
        let Some(reference) = key.as_str().and_then(|key| expected.get(key)) else {
            return false;
        };
        match (value, reference) {
            (Variant::Vector3(a), Variant::Vector3(b))
                if key.as_str() == Some("spin") || key.as_str() == Some("velocity") =>
            {
                let scale = b.values.into_iter().map(Real::abs).fold(1.0, Real::max);
                a.values
                    .into_iter()
                    .zip(b.values)
                    .any(|(a, b)| exceeds_strike_row(a, b, scale))
            }
            (Variant::Float(a), Variant::Float(b)) => exceeds_strike_row(a.value(), b.value(), 1.0),
            _ => false,
        }
    });
    if !needs_bound {
        return Ok(());
    }
    let bounds = strike_bounds(request, ground, actual);
    let velocity = vec3(actual, "velocity");
    let spin = vec3(actual, "spin");
    let spin_bound = Vec3::new(bounds.spin[0], bounds.spin[1], bounds.spin[2]).length();
    let direction = |radius: Real, length: Real| {
        if radius < length {
            fm::asin(radius / length)
        } else {
            0.0
        }
    };
    let heading = direction(bounds.velocity, Vec2::new(velocity.x, velocity.z).length());
    let loft = direction(bounds.velocity, velocity.length());
    let face = direction(
        bounds.normal,
        fm::cos(number(actual, "dynamic_loft_rad")).abs(),
    );
    let spin_axis = direction(spin_bound, spin.length());
    let contact_normal = direction(bounds.normal, 1.0);
    let spin_loft = spin_loft_rounding_bound(number(actual, "spin_loft_rad")) + contact_normal;
    for (key, bound) in [
        ("ball_speed_mps", bounds.velocity),
        ("launch_angle_rad", loft),
        ("launch_direction_rad", heading),
        ("spin_rate_rad_s", spin_bound),
        ("spin_axis_rad", spin_axis),
        (
            "smash_factor",
            bounds.velocity / request.delivery.club_speed_mps.max(1.0),
        ),
        ("face_angle_rad", face),
        ("dynamic_loft_rad", contact_normal),
        ("face_to_path_rad", face),
        ("spin_loft_rad", spin_loft),
        ("strike_up_m", bounds.face_up),
        ("velocity", bounds.velocity),
        ("spin", spin_bound),
    ] {
        let (Some(reference), Some(computed)) = (expected.get(key), actual.get_mut(key)) else {
            continue;
        };
        let accept = |reference: Real, computed: Real, scale: Real, bound: Real| {
            let difference = (reference - computed).abs();
            let error = if key == "spin_axis_rad" {
                difference.min((std::f64::consts::TAU - difference).abs())
            } else {
                difference
            };
            let ordinary = 1e-6_f64
                .max(1e-5 * reference.abs().max(computed.abs()))
                .max(EIGHT_SINGLE_PRECISION_STEPS * scale.max(1.0));
            if difference > ordinary && (error <= ordinary || (bound.is_finite() && error <= bound))
            {
                println!(
                    "case {id} {key}: {error:.3e} within {:.3e} from contact operand rounding",
                    ordinary.max(bound)
                );
                !drift
            } else {
                false
            }
        };
        match (reference, computed) {
            (Variant::Vector3(reference), Variant::Vector3(computed)) => {
                let scale = reference
                    .values
                    .into_iter()
                    .map(Real::abs)
                    .fold(1.0, Real::max);
                for i in 0..3 {
                    let bound = if key == "spin" { bounds.spin[i] } else { bound };
                    if accept(reference.values[i], computed.values[i], scale, bound) {
                        computed.values[i] = reference.values[i];
                    }
                }
            }
            (Variant::Float(reference), Variant::Float(computed))
                if accept(reference.value(), computed.value(), 1.0, bound) =>
            {
                *computed = *reference;
            }
            _ => {}
        }
    }
    Ok(())
}

pub(in crate::corpus) fn compare_shot_launch(
    expected: &Dict,
    actual: &mut Dict,
    compare: impl FnOnce(&Dict, &mut Dict) -> Result<(), String>,
) -> Result<(), String> {
    use golf_sim_physics::values::EventKind;
    let Some(expected_launch) = expected.get("launch").and_then(Variant::as_dict) else {
        return Ok(());
    };
    let Some(Variant::Dict(launch)) = actual.get_mut("launch") else {
        return Ok(());
    };

    let original = launch.clone();
    compare(expected_launch, launch)?;
    let corrected = launch.clone();
    let Some(Variant::Array(events)) = actual.get_mut("events") else {
        return Ok(());
    };
    let Some(Variant::Dict(event)) = events.items.first_mut() else {
        return Ok(());
    };
    let Some(expected_event) = expected
        .get("events")
        .and_then(|v| array(v).first())
        .and_then(Variant::as_dict)
    else {
        return Ok(());
    };
    let is_impact = |event: &Dict| {
        matches!(event.get("kind"), Some(Variant::Int(kind))
        if kind.value() == EventKind::Impact as i64)
    };
    if !is_impact(event) || !is_impact(expected_event) {
        return Ok(());
    }
    for key in ["velocity", "spin"] {
        if original.get(key) == event.get(key)
            && expected_launch.get(key) == expected_event.get(key)
            && let Some(value) = corrected.get(key)
        {
            *event.get_mut(key).unwrap() = value.clone();
        }
    }
    Ok(())
}

fn replay(case: &Dict, course: Option<&Course>, drift: bool) -> Result<Variant, String> {
    let call = field(case, "call").as_str().unwrap();
    let input = dict(field(case, "input"));
    let sub = |key| dict(field(input, key));
    let result = match call {
        "club_impact.head_orientation" => {
            let head = impact::head_orientation(
                &club(sub("club")),
                &delivery(sub("d")),
                vec3(input, "f"),
                vec3(input, "s"),
                number(input, "close_rad"),
            );
            record! {
                normal: vector(head.normal),
                face_up: vector(head.face_up),
                toe: vector(head.toe),
                forward: vector(head.forward),
                up: vector(head.up),
            }
        }
        "club_impact.sole_height_for_strike" => impact::sole_height_for_strike(
            &club(sub("club")),
            &ball(sub("ball")),
            &delivery(sub("d")),
            &lie(sub("lie")),
            number(input, "up"),
        )
        .into(),
        "strike_contact.before_ball" => {
            let mut request = ShotRequest::new(
                club(sub("club")),
                BallSpec {
                    radius_m: number(input, "ball_radius"),
                    ..BallSpec::default()
                },
                delivery(sub("d")),
                lie(sub("lie")),
            );
            request.position = vec3(input, "ground_point");
            let pre = with_ground(input, course, |ground| {
                strike::before_ball(&request, vec3(input, "f"), vec3(input, "s"), ground)
            })?;
            record! {
                club_speed_mps: pre.club_speed_mps,
                face_close_rad: pre.face_close_rad,
                interference: pre.interference,
                cushion: pre.cushion,
                sole_height_m: pre.sole_height_m,
                low_point_ahead_m: pre.low_point_ahead_m,
                low_point_height_m: pre.low_point_height_m,
                entry_behind_m: pre.entry_behind_m,
                turf_start: turf_point(pre.turf_start),
                turf_end: turf_point(pre.turf_end),
                turf_depth_m: pre.turf_depth_m,
                sand_fraction: pre.sand_fraction,
                sand_blend: pre.sand_blend,
                sand_transfer: pre.sand_transfer,
                sand_bounce_rad: pre.sand_bounce_rad,
                sand_opening: pre.sand_opening,
                direct_sand_access: pre.direct_sand_access,
                sand_spin_transfer: pre.sand_spin_transfer,
                grass_transfer: pre.grass_transfer,
                grass_film: pre.grass_film,
            }
        }
        "club_impact.strike" => {
            let req = sub("req");
            let part = |key| dict(field(req, key));
            let mut request = ShotRequest::new(
                club(part("club")),
                ball(part("ball")),
                delivery(part("delivery")),
                lie(part("lie")),
            );
            request.position = vec3(req, "position");
            request.aim = vec3(req, "aim");
            let temperature = number(req, "ball_temperature_c");
            request.ball_temperature_c = (!temperature.is_nan()).then_some(temperature);
            let air = AirSample {
                temperature_c: number(sub("air"), "temperature_c"),
                ..AirSample::default()
            };
            with_ground(input, course, |ground| {
                let mut output =
                    launch_output(impact::strike(&request, &air, ground, &mut NoProbe));
                let Variant::Dict(actual) = &mut output else {
                    unreachable!()
                };
                if input.get("ground_reads").is_some() {
                    check_strike_fields(dict(field(case, "output")), actual)?;
                } else {
                    compare_launch_rounding(
                        case,
                        dict(field(case, "output")),
                        actual,
                        &request,
                        ground,
                        drift,
                    )?;
                }
                Ok::<_, String>(output)
            })??
        }
        _ => return Err(format!("unknown impact call {call}")),
    };
    Ok(result)
}

pub const SPIN_LOFT_CASES: [&str; 3] = [
    "suite/test_contact/test_difficult_lie_launches_change_smoothly/001343",
    "suite/test_contact/test_difficult_lie_launches_change_smoothly/001423",
    "suite/test_contact/test_difficult_lie_launches_change_smoothly/001427",
];

fn spin_loft_rounding_bound(angle: Real) -> Real {
    let dot = fm::cos(angle);
    let lower = fm::acos((dot + EIGHT_SINGLE_PRECISION_STEPS).min(1.0));
    let upper = fm::acos((dot - EIGHT_SINGLE_PRECISION_STEPS).max(-1.0));
    (angle - lower).max(upper - angle)
}

pub(in crate::corpus) fn compare(
    case: &Dict,
    course: Option<&Course>,
    drift_report: bool,
) -> Result<Variant, String> {
    let mut actual = replay(case, course, drift_report)?;
    if field(case, "call").as_str() == Some("club_impact.strike") {
        let key = "spin_loft_rad";
        let expected = dict(field(case, "output"));
        let Variant::Dict(actual_fields) = &mut actual else {
            return Err("expected launch output".into());
        };
        let angle = number(actual_fields, key);
        let recorded = number(expected, key);
        let drift = (recorded - angle).abs();
        let normal = 1e-6_f64.max(1e-5 * recorded.abs().max(angle.abs()));
        let bound = spin_loft_rounding_bound(angle);
        if (normal..=bound).contains(&drift) {
            let id = field(case, "id").as_str().unwrap();
            println!(
                "case {id} {key}: {drift:.3e} rad from acos of the rounded direction dot product (bound {bound:.3e} rad)"
            );
            if !drift_report {
                *actual_fields.get_mut(key).unwrap() = field(expected, key).clone();
            }
        }
    }
    Ok(actual)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bunker_rounding_requires_both_contact_branches_and_the_core_blend() {
        use golf_sim_base::{ClubId, digest::object, num::deg_to_rad};

        let club = t::standard_club(ClubId::SandWedge);
        let ball = t::standard_ball();
        let lie = Lie {
            surface: Surface::Bunker,
            sand_firmness: 0.5,
            ball_height_m: -0.001,
            ..Lie::default()
        };
        let delivery = Delivery {
            club_speed_mps: 28.0,
            dynamic_loft_rad: deg_to_rad(48.0),
            attack_rad: deg_to_rad(-6.0),
            sole_height_m: -0.003,
            ..Delivery::default()
        };
        let mut request = ShotRequest::new(club, ball, delivery, lie);
        request.position = Vec3::new(0.0, 256.0, 0.0);
        request.aim = Vec3::new(0.0, 0.0, -1.0);
        let ground = PlaneGround::new(
            GroundSample {
                surface: Surface::Bunker,
                ..GroundSample::default()
            },
            request.position,
        );
        let launch = impact::strike(&request, &AirSample::default(), &ground, &mut NoProbe);
        assert_eq!(launch.contact, Contact::Face);
        let actual = launch_output(launch);
        let actual = dict(&actual);
        let bounds = strike_bounds(&request, &ground, actual);
        assert!(bounds.face_up > 2e-6);
        assert!(bounds.velocity > 0.002);

        let case = object([("id", "bunker-blend".into())]);
        let mut reference = actual.clone();
        let shifted = launch.velocity + Vec3::new(0.0, 0.002, 0.0);
        *reference.get_mut("velocity").unwrap() = vector(shifted);
        let compare_velocity = |mut computed: Dict| {
            compare_launch_rounding(
                dict(&case),
                &reference,
                &mut computed,
                &request,
                &ground,
                false,
            )
            .unwrap();
            vec3(&computed, "velocity")
        };
        assert_eq!(compare_velocity(actual.clone()), shifted);

        let mut wrong_core = actual.clone();
        let mutated = launch.velocity + Vec3::new(0.0, -0.002, 0.0);
        *wrong_core.get_mut("velocity").unwrap() = vector(mutated);
        assert_eq!(compare_velocity(wrong_core), mutated);

        let key = "ball_speed_mps";
        let mutated = launch.readout.ball_speed_mps + 0.002;
        let mut wrong_core = actual.clone();
        *wrong_core.get_mut(key).unwrap() = mutated.into();
        compare_launch_rounding(
            dict(&case),
            actual,
            &mut wrong_core,
            &request,
            &ground,
            false,
        )
        .unwrap();
        assert_eq!(number(&wrong_core, key), mutated);

        for key in [
            "launch_angle_rad",
            "launch_direction_rad",
            "spin_rate_rad_s",
            "spin_axis_rad",
            "smash_factor",
        ] {
            let mut wrong_core = actual.clone();
            *wrong_core.get_mut(key).unwrap() = (number(actual, key) + 0.002).into();
            assert_eq!(compare_velocity(wrong_core), launch.velocity, "{key}");
        }
    }

    #[test]
    fn the_reference_divot_sampler_rounds_plan_coordinates_before_the_edge_test() {
        let back = 0.0625;
        let ground = DivotGround { depth: 0.01, back };
        assert_eq!(ground.sample_ground(0.0, back - 1e-7).height_m, -0.01);
        assert_eq!(ground.sample_ground(0.0, back + 1e-9).height_m, -0.01);
        assert_eq!(ground.sample_ground(0.0, back + 1e-7).height_m, 0.0);
    }

    #[test]
    fn a_nonfinite_spin_or_wrong_contact_cannot_use_the_rounding_bound() {
        let launch = Launch::default();
        let expected = launch_output(launch);
        let actual = launch_output(Launch {
            spin: Vec3::new(0.0, Real::INFINITY, 0.0),
            ..launch
        });
        assert_eq!(
            check_strike_fields(dict(&expected), dict(&actual)).unwrap_err(),
            "core spin is not finite"
        );
        let actual = launch_output(Launch {
            contact: Contact::TopEdge,
            ..launch
        });
        assert_eq!(
            check_strike_fields(dict(&expected), dict(&actual)).unwrap_err(),
            "contact kind differs"
        );
    }

    #[test]
    fn contact_height_limits_the_core_strike_up_error() {
        use golf_sim_base::{ClubId, digest::object};
        use golf_sim_physics::tunables::{standard_ball, standard_club};
        let club = standard_club(ClubId::Iron7);
        let ball = standard_ball();
        let lie = Lie {
            ball_height_m: 0.004,
            ..Lie::default()
        };
        let mut delivery = Delivery {
            club_speed_mps: 30.0,
            dynamic_loft_rad: 0.38,
            ..Delivery::default()
        };
        delivery.sole_height_m = impact::sole_height_for_strike(&club, &ball, &delivery, &lie, 0.0);
        let mut request = ShotRequest::new(club, ball, delivery, lie);
        request.position = Vec3::new(0.0, 256.0, 0.0);
        request.aim = Vec3::new(0.0, 0.0, -1.0);
        let ground = PlaneGround::new(GroundSample::default(), request.position);
        let launch = impact::strike(&request, &AirSample::default(), &ground, &mut NoProbe);
        assert_eq!(launch.contact, Contact::Face);
        let output = launch_output(launch);
        let actual = dict(&output);
        let radius = strike_bounds(&request, &ground, actual).face_up;
        assert!(radius > 2e-6);
        let case = object([("id", "synthetic-strike".into())]);
        let checked_up = |delta| {
            let mut reference = actual.clone();
            *reference.get_mut("strike_up_m").unwrap() = Variant::from(launch.strike_up_m + delta);
            let mut computed = actual.clone();
            compare_launch_rounding(
                dict(&case),
                &reference,
                &mut computed,
                &request,
                &ground,
                false,
            )
            .unwrap();
            number(&computed, "strike_up_m")
        };
        assert_eq!(
            checked_up(0.75 * radius),
            launch.strike_up_m + 0.75 * radius
        );
        assert_eq!(checked_up(2.0 * radius), launch.strike_up_m);
    }

    #[test]
    fn spin_loft_bound_tightens_away_from_parallel_directions() {
        let dot = 1.0 - EIGHT_SINGLE_PRECISION_STEPS * 0.5;
        let drift = fm::acos(dot);
        assert!(drift > 1e-6);
        assert!(drift <= spin_loft_rounding_bound(0.0));
        assert!(spin_loft_rounding_bound(std::f64::consts::FRAC_PI_2) < drift);
    }
}
