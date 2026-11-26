//! Rigid-body club–ball impact, including the face curvature and the clubhead's inertia.

use golf_sim_base::{
    Basis, Real, Surface, Vec2, Vec3, fm, log,
    num::{clampf, lerpf, smoothstep},
    trace::{Phase, Probe, Source},
};

use crate::{
    samplers::{AirSample, GroundSampler},
    strike::{self, PreImpact},
    tunables as t,
    values::{
        BallSpec, ClubCategory, ClubSpec, Contact, Delivery, Launch, LaunchReadout, Lie,
        ShotRequest, UP,
    },
};

const FORWARD: Vec3 = Vec3::new(0.0, 0.0, -1.0);
const RIGHT: Vec3 = Vec3::new(1.0, 0.0, 0.0);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Head {
    pub normal: Vec3,
    pub face_up: Vec3,
    pub toe: Vec3,
    pub forward: Vec3,
    pub up: Vec3,
}

pub fn head_orientation(
    club: &ClubSpec,
    delivery: &Delivery,
    forward: Vec3,
    right: Vec3,
    close_rad: Real,
) -> Head {
    let lie = -delivery.dynamic_lie_rad;
    let face = -(delivery.face_rad - close_rad);
    let orient = |axis: Vec3, loft| {
        axis.rotated(right, loft)
            .rotated(forward, lie)
            .rotated(UP, face)
    };
    let head_loft = delivery.dynamic_loft_rad - club.loft_rad;
    Head {
        normal: orient(forward, delivery.dynamic_loft_rad),
        face_up: orient(UP, delivery.dynamic_loft_rad),
        toe: right.rotated(forward, lie).rotated(UP, face),
        forward: orient(forward, head_loft),
        up: orient(UP, head_loft),
    }
}

pub fn sole_height_for_strike(
    club: &ClubSpec,
    ball: &BallSpec,
    delivery: &Delivery,
    lie: &Lie,
    up_m: Real,
) -> Real {
    let head = head_orientation(club, delivery, FORWARD, RIGHT, 0.0);
    let height = if lie.surface == Surface::Bunker {
        lie.ball_height_m.max(0.0)
    } else {
        lie.ball_height_m
    };
    Face {
        club,
        head,
        radius: ball.radius_m,
    }
    .sole_height(delivery.strike_toe_m, up_m, ball.radius_m + height)
}

pub fn strike<P: Probe>(
    request: &ShotRequest,
    air: &AirSample,
    ground: &(impl GroundSampler + ?Sized),
    probe: &mut P,
) -> Launch {
    let club = &request.club;
    let ball = &request.ball;
    let d = &request.delivery;
    let lie = &request.lie;
    let (forward, right, approach) = {
        let _span = log::delivery().entered();
        let forward = Vec3::new(request.aim.x, 0.0, request.aim.z).normalized();
        let right = forward.cross(UP);
        let approach = (forward * fm::cos(d.path_rad) + right * fm::sin(d.path_rad))
            * fm::cos(d.attack_rad)
            + UP * fm::sin(d.attack_rad);
        if P::ENABLED {
            trace_delivery(probe, request, forward);
        }
        (forward, right, approach)
    };
    let mut launch = {
        let _span = log::impact().entered();
        let pre = strike::before_ball(request, forward, right, ground);
        let face = Face {
            club,
            head: head_orientation(club, d, forward, right, pre.face_close_rad),
            radius: ball.radius_m,
        };
        // Local coordinates preserve the lever arms at large world positions.
        let center = UP * (ball.radius_m + lie.ball_height_m);
        let geometry = face.contact(center, approach, d.strike_toe_m, pre.sole_height_m);
        let club_speed = pre.club_speed_mps * (1.0 + d.strike_toe_m / club.arc_radius_m);
        let ball_temperature_c = request.ball_temperature_c.unwrap_or(air.temperature_c);
        let normal = geometry.normal;
        let face_angle_rad = fm::atan2(normal.dot(right), normal.dot(forward));
        let mut launch = Launch {
            aim: forward,
            position: request.position + center,
            contact: if pre.sand_fraction * pre.sand_blend >= 0.5 {
                Contact::Sand
            } else {
                geometry.kind
            },
            strike_toe_m: geometry.strike_toe_m,
            strike_up_m: geometry.strike_up_m,
            ball_temperature_c,
            club_speed_mps: d.club_speed_mps,
            club_speed_at_ball_mps: pre.club_speed_mps,
            club_path_rad: d.path_rad,
            attack_angle_rad: d.attack_rad,
            low_point_ahead_m: pre.low_point_ahead_m,
            low_point_height_m: pre.low_point_height_m,
            turf_entry_behind_m: pre.entry_behind_m,
            turf_contact_start: pre.turf_start,
            turf_contact_end: pre.turf_end,
            turf_contact_depth_m: pre.turf_depth_m,
            interference: pre.interference,
            face_angle_rad,
            dynamic_loft_rad: fm::asin(clampf(normal.y, -1.0, 1.0)),
            face_to_path_rad: face_angle_rad - d.path_rad,
            spin_loft_rad: fm::acos(clampf(normal.dot(approach), -1.0, 1.0)),
            ..Launch::default()
        };
        let club_velocity = approach * club_speed;
        if geometry.kind != Contact::Miss {
            let (velocity, spin) =
                face.collide(ball, &geometry, club_velocity, ball_temperature_c, &pre);
            launch.velocity = velocity * pre.direct_sand_access * pre.grass_transfer;
            launch.spin = spin * pre.direct_sand_access * pre.grass_transfer;
        }
        if pre.sand_fraction > 0.0 && pre.sand_blend > 0.0 {
            // Use the unblocked collision for both ends of the blend before charging for sand.
            let supported = face.contact(
                UP * (ball.radius_m + lie.ball_height_m.max(0.0)),
                approach,
                d.strike_toe_m,
                pre.sole_height_m.max(0.0),
            );
            if supported.kind != Contact::Miss {
                let (velocity, spin) = face.collide(
                    ball,
                    &supported,
                    club_velocity,
                    ball_temperature_c,
                    &PreImpact::default(),
                );
                let (splash_velocity, splash_spin) = splash(face.head, velocity, spin, &pre);
                let access = lerpf(pre.direct_sand_access, 1.0, pre.sand_fraction);
                let sand_velocity =
                    velocity.lerp(splash_velocity, pre.sand_fraction) * pre.sand_transfer * access;
                let sand_spin = spin.lerp(splash_spin, pre.sand_fraction)
                    * pre.sand_transfer
                    * access
                    * pre.sand_spin_transfer;
                launch.velocity = launch.velocity.lerp(sand_velocity, pre.sand_blend);
                launch.spin = launch.spin.lerp(sand_spin, pre.sand_blend);
            }
        }
        if club.category == ClubCategory::Driver {
            let limit = t::DRIVER_SMASH_LIMIT * d.club_speed_mps;
            if launch.velocity.length_squared() > limit * limit {
                let keep = limit / launch.velocity.length();
                launch.velocity = launch.velocity * keep;
                launch.spin = launch.spin * keep;
            }
        }
        if P::ENABLED {
            trace_impact(probe, request, &pre, &launch);
        }
        launch
    };
    {
        let _span = log::launch().entered();
        let v = launch.velocity;
        let horizontal = Vec2::new(v.dot(forward), v.dot(right));
        let side = (forward * horizontal.x + right * horizontal.y)
            .normalized()
            .cross(UP);
        let horizontal_spin = Vec2::new(launch.spin.x, launch.spin.z).length();
        let signed_spin = if launch.spin.dot(side) < 0.0 {
            -horizontal_spin
        } else {
            horizontal_spin
        };
        launch.readout = LaunchReadout {
            ball_speed_mps: v.length(),
            launch_angle_rad: fm::atan2(v.y, horizontal.length()),
            launch_direction_rad: fm::atan2(horizontal.y, horizontal.x),
            spin_rate_rad_s: launch.spin.length(),
            spin_axis_rad: fm::atan2(-launch.spin.y, signed_spin),
        };
        if d.club_speed_mps > 0.0 {
            launch.smash_factor = launch.readout.ball_speed_mps / d.club_speed_mps;
        }
        if P::ENABLED {
            trace_launch(probe, &launch);
        }
    }
    launch
}

struct Geometry {
    point: Vec3,
    normal: Vec3,
    face_center: Vec3,
    strike_toe_m: Real,
    strike_up_m: Real,
    kind: Contact,
    edge_distance: Real,
}

struct Face<'a> {
    club: &'a ClubSpec,
    head: Head,
    radius: Real,
}

impl Face<'_> {
    fn offset(&self, toe: Real, up: Real) -> Vec3 {
        let mut sag = 0.0;
        if self.club.bulge_radius_m > 0.0 {
            sag += toe * toe / (2.0 * self.club.bulge_radius_m);
        }
        if self.club.roll_radius_m > 0.0 {
            sag += up * up / (2.0 * self.club.roll_radius_m);
        }
        self.head.face_up * up + self.head.toe * toe - self.head.normal * sag
    }

    fn normal(&self, toe: Real, up: Real) -> Vec3 {
        let mut normal = self.head.normal;
        if self.club.bulge_radius_m > 0.0 {
            normal += self.head.toe * (toe / self.club.bulge_radius_m);
        }
        if self.club.roll_radius_m > 0.0 {
            normal += self.head.face_up * (up / self.club.roll_radius_m);
        }
        normal.normalized()
    }

    fn sole_height(&self, toe: Real, up: Real, center_y: Real) -> Real {
        let leading = self.offset(0.0, -self.club.face_center_height_m);
        center_y - self.normal(toe, up).y * self.radius - self.offset(toe, up).y + leading.y
    }

    fn contact(&self, center: Vec3, approach: Vec3, toe: Real, sole_height: Real) -> Geometry {
        let club = self.club;
        // Find sphere tangency to the extended face before testing the finite edges.
        let mut up = 0.0;
        for _ in 0..6 {
            let error = self.sole_height(toe, up, center.y) - sole_height;
            let derivative = (self.sole_height(toe, up + 0.0001, center.y)
                - self.sole_height(toe, up - 0.0001, center.y))
                / 0.0002;
            if derivative.abs() < 1e-5 {
                break;
            }
            up -= error / derivative;
        }
        let normal = self.normal(toe, up);
        let point = center - normal * self.radius;
        let mut geometry = Geometry {
            normal,
            point,
            face_center: point - self.offset(toe, up),
            strike_toe_m: clampf(toe, -club.face_half_width_m, club.face_half_width_m),
            strike_up_m: clampf(
                up,
                -club.face_center_height_m,
                club.face_height_m - club.face_center_height_m,
            ),
            kind: Contact::Face,
            edge_distance: 0.0,
        };
        if geometry.strike_toe_m == toe && geometry.strike_up_m == up {
            return geometry;
        }

        geometry.edge_distance =
            Vec2::new(toe - geometry.strike_toe_m, up - geometry.strike_up_m).length();
        geometry.kind = if up < geometry.strike_up_m {
            Contact::LeadingEdge
        } else if up > geometry.strike_up_m {
            Contact::TopEdge
        } else {
            Contact::SideEdge
        };
        let edge_offset = self.offset(geometry.strike_toe_m, geometry.strike_up_m);
        let edge = geometry.face_center + edge_offset;
        let delta = center - edge;
        let along = delta.dot(approach);
        let off_axis = delta - approach * along;
        let discriminant = self.radius * self.radius - off_axis.length_squared();
        if discriminant <= 0.0 {
            geometry.kind = Contact::Miss;
            return geometry;
        }
        geometry.point = edge + approach * (along - fm::sqrt(discriminant));
        geometry.normal = (center - geometry.point).normalized();
        geometry.face_center = geometry.point - edge_offset;
        geometry
    }

    fn collide(
        &self,
        ball: &BallSpec,
        geometry: &Geometry,
        club_velocity: Vec3,
        ball_temperature_c: Real,
        pre: &PreImpact,
    ) -> (Vec3, Vec3) {
        let club = self.club;
        let head = self.head;
        let normal = geometry.normal;
        let incoming = -club_velocity;
        let normal_speed = incoming.dot(normal);
        if normal_speed >= 0.0 {
            return (Vec3::ZERO, Vec3::ZERO);
        }

        let r_ball = -normal * ball.radius_m;
        let cg = geometry.face_center - head.forward * club.cg_depth_m
            + head.up * club.cg_up_m
            + head.toe * club.cg_toe_m;
        let r_club = geometry.point - cg;
        let ball_inertia = ball.moment_of_inertia();
        let inverse_club_inertia = |v: Vec3| {
            head.forward * (head.forward.dot(v) / club.moi_forward)
                + head.up * (head.up.dot(v) / club.moi_vertical)
                + head.toe * (head.toe.dot(v) / club.moi_heel_toe)
        };
        // This maps an impulse to the change in relative contact velocity of both bodies.
        let response = |impulse: Vec3| {
            impulse * (1.0 / ball.mass_kg + 1.0 / club.head_mass_kg)
                - r_ball.cross(r_ball.cross(impulse)) / ball_inertia
                - r_club.cross(inverse_club_inertia(r_club.cross(impulse)))
        };
        let [x, y, z] = [RIGHT, UP, -FORWARD].map(response);
        let effective_mass = Basis {
            rows: [
                Vec3::new(x.x, y.x, z.x),
                Vec3::new(x.y, y.y, z.y),
                Vec3::new(x.z, y.z, z.z),
            ],
        };
        let slip = incoming - normal * normal_speed;
        let e = restitution(
            club,
            -normal_speed,
            geometry,
            ball_temperature_c,
            pre.cushion,
        );
        let keep = 1.0 - pre.interference * t::INTERFERENCE_TANGENTIAL_LOSS;
        let toe = head.toe.slide(normal).normalized();
        let up = normal.cross(toe);
        let friction = club.face_friction
            * (1.0 - pre.interference * t::INTERFERENCE_FRICTION_LOSS)
            * fm::exp(-t::GRASS_FILM_FRICTION_DECAY * pre.grass_film);
        let up_cor = club.tangential_restitution_up
            + club.tangential_restitution_speed_slope
                * clampf(
                    t::COR_REFERENCE_SPEED + normal_speed,
                    0.0,
                    t::TANGENTIAL_SPEED_RANGE_MPS,
                );
        let target = normal * (-e * normal_speed)
            - toe * (slip.dot(toe) * club.tangential_restitution_toe * keep)
            - up * (slip.dot(up) * up_cor * keep);
        let mut impulse = effective_mass.inverse().xform(target - incoming);
        let normal_impulse = impulse.dot(normal);
        let tangent = impulse - normal * normal_impulse;
        if normal_impulse <= 0.0 || tangent.length() > friction * normal_impulse {
            // Coupled clubhead inertia can require friction even at zero incoming slip.
            let friction_dir = if slip.length_squared() > 1e-12 {
                -slip.normalized()
            } else {
                tangent.normalized()
            };
            let mut direction = normal + friction_dir * friction;
            let mut denominator = normal.dot(effective_mass.xform(direction));
            if denominator <= 0.0 {
                direction = normal;
                denominator = normal.dot(effective_mass.xform(normal));
            }
            impulse = direction * (-(1.0 + e) * normal_speed / denominator);
        }
        (impulse / ball.mass_kg, r_ball.cross(impulse) / ball_inertia)
    }
}

fn restitution(
    club: &ClubSpec,
    normal_speed: Real,
    geometry: &Geometry,
    temperature: Real,
    cushion: Real,
) -> Real {
    let mut e = club.cor;
    if !matches!(club.category, ClubCategory::Driver | ClubCategory::Putter) {
        let slope = t::COR_SPEED_SLOPE + club.cor_speed_slope_delta;
        e = (e + slope * (t::COR_REFERENCE_SPEED - normal_speed)).min(t::COR_LOW_SPEED_MAX);
    }
    let toe = geometry.strike_toe_m;
    let up = geometry.strike_up_m;
    e *= 1.0 - club.cor_falloff_toe * toe * toe - club.cor_falloff_up * up * up;
    e += t::COR_PER_DEGREE_C * (temperature - t::COR_TEMPERATURE_REFERENCE_C);
    e *= lerpf(
        1.0,
        t::EDGE_COR_FACTOR,
        smoothstep(0.0, t::EDGE_COR_BLEND_M, geometry.edge_distance),
    );
    e *= 1.0 - cushion * t::INTERFERENCE_COR_LOSS;
    clampf(e, 0.05, 0.95)
}

fn splash(head: Head, clean_velocity: Vec3, clean_spin: Vec3, pre: &PreImpact) -> (Vec3, Vec3) {
    let direction = clean_velocity
        .normalized()
        .lerp(head.normal, t::SAND_FACE_WEIGHT);
    let direction =
        (direction + UP * (fm::sin(pre.sand_bounce_rad) * t::SAND_BOUNCE_LAUNCH_GAIN)).normalized();
    let speed = clean_velocity.length() * (1.0 - t::SAND_OPEN_SPEED_LOSS * pre.sand_opening);
    let spin = clean_spin * (t::SAND_SPIN_KEEP * (1.0 - t::SAND_OPEN_SPIN_LOSS * pre.sand_opening));
    (direction * speed, spin)
}

fn trace_delivery(probe: &mut impl Probe, request: &ShotRequest, aim: Vec3) {
    probe.begin_phase(Phase::Delivery);
    let d = &request.delivery;
    for (name, value, unit) in [
        ("club_speed", d.club_speed_mps, "m/s"),
        ("path", d.path_rad, "rad"),
        ("attack", d.attack_rad, "rad"),
        ("face", d.face_rad, "rad"),
        ("dynamic_loft", d.dynamic_loft_rad, "rad"),
        ("dynamic_lie", d.dynamic_lie_rad, "rad"),
        ("strike_toe", d.strike_toe_m, "m"),
        ("sole_height", d.sole_height_m, "m"),
    ] {
        probe.value(name, value, unit, Source::Computed);
    }
    for (name, value, unit) in [
        ("ball_height_m", request.lie.ball_height_m, "m"),
        ("grass_height_m", request.lie.grass_height_m, "m"),
        ("grass_density", request.lie.grass_density, "1"),
        ("sand_firmness", request.lie.sand_firmness, "1"),
    ] {
        probe.value(name, value, unit, Source::Turf { name: name.into() });
    }
    trace_vector(probe, "aim", aim, "1");
    let club = &request.club;
    for (name, value, unit) in [
        ("arc_radius_m", club.arc_radius_m, "m"),
        ("loft_rad", club.loft_rad, "rad"),
        ("sole_width_m", club.sole_width_m, "m"),
        ("sole_bounce_rad", club.sole_bounce_rad, "rad"),
    ] {
        tunable(probe, name, value, unit);
    }
}

fn tunable(probe: &mut impl Probe, name: &str, value: Real, unit: &str) {
    probe.value(name, value, unit, Source::Tunable { name: name.into() });
}

fn trace_impact(probe: &mut impl Probe, request: &ShotRequest, pre: &PreImpact, launch: &Launch) {
    probe.begin_phase(Phase::Impact);
    for (name, value, unit) in [
        ("club_speed_at_ball", pre.club_speed_mps, "m/s"),
        ("face_close", pre.face_close_rad, "rad"),
        ("sole_height_at_ball", pre.sole_height_m, "m"),
        ("low_point_ahead", pre.low_point_ahead_m, "m"),
        ("low_point_height", pre.low_point_height_m, "m"),
        ("turf_entry_behind", pre.entry_behind_m, "m"),
        ("turf_contact_depth", pre.turf_depth_m, "m"),
        ("interference", pre.interference, "1"),
        ("cushion", pre.cushion, "1"),
        ("grass_transfer", pre.grass_transfer, "1"),
        ("grass_film", pre.grass_film, "1"),
        ("sand_fraction", pre.sand_fraction, "1"),
        ("sand_blend", pre.sand_blend, "1"),
        ("sand_transfer", pre.sand_transfer, "1"),
        ("sand_bounce", pre.sand_bounce_rad, "rad"),
        ("sand_opening", pre.sand_opening, "1"),
        ("direct_sand_access", pre.direct_sand_access, "1"),
        ("sand_spin_transfer", pre.sand_spin_transfer, "1"),
        ("ball_temperature", launch.ball_temperature_c, "deg C"),
        ("strike_toe", launch.strike_toe_m, "m"),
        ("strike_up", launch.strike_up_m, "m"),
        ("face_angle", launch.face_angle_rad, "rad"),
        ("dynamic_loft", launch.dynamic_loft_rad, "rad"),
        ("face_to_path", launch.face_to_path_rad, "rad"),
        ("spin_loft", launch.spin_loft_rad, "rad"),
        ("contact", Real::from(launch.contact as u8), "1"),
    ] {
        probe.value(name, value, unit, Source::Computed);
    }
    if let Some(start) = pre.turf_start {
        trace_vector(probe, "turf_contact_start", start, "m");
    }
    if let Some(end) = pre.turf_end {
        trace_vector(probe, "turf_contact_end", end, "m");
    }
    let club = &request.club;
    for (name, value, unit) in [
        ("cor", club.cor, "1"),
        ("cor_falloff_toe", club.cor_falloff_toe, "1/m²"),
        ("cor_falloff_up", club.cor_falloff_up, "1/m²"),
        ("face_friction", club.face_friction, "1"),
        (
            "tangential_restitution_toe",
            club.tangential_restitution_toe,
            "1",
        ),
        (
            "tangential_restitution_up",
            club.tangential_restitution_up,
            "1",
        ),
        ("head_mass_kg", club.head_mass_kg, "kg"),
        ("ball_mass_kg", request.ball.mass_kg, "kg"),
        ("ball_radius_m", request.ball.radius_m, "m"),
        ("ball_inertia_factor", request.ball.inertia_factor, "1"),
        ("bulge_radius_m", club.bulge_radius_m, "m"),
        ("roll_radius_m", club.roll_radius_m, "m"),
        ("face_center_height_m", club.face_center_height_m, "m"),
        ("face_height_m", club.face_height_m, "m"),
        ("face_half_width_m", club.face_half_width_m, "m"),
        ("cg_depth_m", club.cg_depth_m, "m"),
        ("cg_up_m", club.cg_up_m, "m"),
        ("cg_toe_m", club.cg_toe_m, "m"),
        ("moi_forward", club.moi_forward, "kg m²"),
        ("moi_vertical", club.moi_vertical, "kg m²"),
        ("moi_heel_toe", club.moi_heel_toe, "kg m²"),
        (
            "GRASS_CLOSE_BY_CATEGORY",
            t::GRASS_CLOSE_BY_CATEGORY[club.category as usize],
            "1",
        ),
    ] {
        tunable(probe, name, value, unit);
    }
    trace_contact_tunables(probe, request);
}

fn trace_contact_tunables(probe: &mut impl Probe, request: &ShotRequest) {
    macro_rules! record {
        ($quantity:literal; $($name:ident: $unit:literal),+ $(,)?) => {
            $(probe.value($quantity, t::$name as Real, $unit,
                Source::Tunable { name: stringify!($name).into() });)+
        };
    }
    record!("club_speed_at_ball";
        ARC_LOOKBACK_M: "m", ARC_SAMPLES: "1", CUT_REFERENCE_SPEED: "m/s", CUT_STATIC_FRACTION: "1",
        HARD_SOLE_FRICTION: "1", HARD_SOLE_IMPULSE_DEPTH_M: "m", SOLE_BOUNCE_DEPTH_RELIEF: "1",
        GRASS_CUT_ENERGY: "J/m³", GRASS_ABOVE_HEAD_SHARE: "1", GRASS_PATH_DEFAULT_DENSITY: "1",
        SAND_CUT_LOW_LOFT_SCALE: "1",
    );
    record!("face_close"; GRASS_FACE_CLOSE_DEG_PER_J: "deg/J", GRASS_FACE_CLOSE_MAX_DEG: "deg");
    record!("interference"; TURF_BLOCK_LOOKBACK_M: "m", GRASS_FULL_BLOCK_M: "m");
    record!("grass_film"; GRASS_FILM_START_M: "m", GRASS_FILM_FULL_M: "m");
    record!("grass_transfer";
        GRASS_COVER_BALL_RADII: "1", GRASS_ESCAPE_START_DEG: "deg", GRASS_ESCAPE_FULL_DEG: "deg",
        GRASS_COVER_LOSS_SHALLOW: "1", GRASS_COVER_LOSS_STEEP: "1",
    );
    record!("restitution";
        COR_REFERENCE_SPEED: "m/s", COR_PER_DEGREE_C: "1/deg C", COR_TEMPERATURE_REFERENCE_C: "deg C",
        EDGE_COR_FACTOR: "1", EDGE_COR_BLEND_M: "m", INTERFERENCE_COR_LOSS: "1",
    );
    record!("tangential_restitution"; TANGENTIAL_SPEED_RANGE_MPS: "m/s", INTERFERENCE_TANGENTIAL_LOSS: "1");
    record!("face_friction"; INTERFERENCE_FRICTION_LOSS: "1", GRASS_FILM_FRICTION_DECAY: "1");
    record!("sole_bounce"; SAND_OPEN_BOUNCE: "1");
    record!("sand_lift"; SAND_LIFT_START_DEG: "deg", SAND_LIFT_FULL_DEG: "deg");

    let club = &request.club;
    tunable(
        probe,
        "tangential_restitution_speed_slope",
        club.tangential_restitution_speed_slope,
        "s/m",
    );
    match club.category {
        ClubCategory::Driver => {
            record!("smash_factor"; DRIVER_SMASH_LIMIT: "1");
        }
        ClubCategory::Putter => {}
        _ => {
            record!("restitution"; COR_SPEED_SLOPE: "s/m", COR_LOW_SPEED_MAX: "1");
            tunable(
                probe,
                "cor_speed_slope_delta",
                club.cor_speed_slope_delta,
                "s/m",
            );
        }
    }
    if request.lie.surface == Surface::Bunker {
        record!("interference"; SAND_FULL_BLOCK_DEPTH_M: "m");
        record!("club_speed_at_ball"; SAND_CUT_SOFT_SCALE: "1", SAND_CUT_FIRM_SCALE: "1");
        record!("sand_fraction";
            SAND_TRANSFER_LENGTH_M: "m", SAND_BLEND_AREA_M2: "m²", SAND_BOUNCE_GAIN: "1",
            SAND_BURIED_BLEND_M: "m", SAND_DIG_CLEARANCE_M: "m",
        );
        record!("sand_transfer";
            SAND_CUSHION_DECAY_M: "m", SAND_EXTRACTION_DECAY_M: "m",
            SAND_TRANSFER_SOFT_SCALE: "1", SAND_TRANSFER_FIRM_SCALE: "1",
        );
        record!("direct_sand_access"; SAND_DIRECT_ACCESS_SOFT_M: "m", SAND_DIRECT_ACCESS_FIRM_M: "m");
        record!("sand_spin_transfer"; SAND_BURIED_SPIN_KEEP: "1");
        record!("splash";
            SAND_FACE_WEIGHT: "1", SAND_BOUNCE_LAUNCH_GAIN: "1", SAND_OPEN_SPEED_LOSS: "1",
            SAND_SPIN_KEEP: "1", SAND_OPEN_SPIN_LOSS: "1",
        );
    } else {
        record!("interference"; TURF_FULL_BLOCK_DEPTH_M: "m");
    }
}

fn trace_launch(probe: &mut impl Probe, launch: &Launch) {
    probe.begin_phase(Phase::Launch);
    for (name, value, unit) in [
        ("ball_speed", launch.readout.ball_speed_mps, "m/s"),
        ("launch_angle", launch.readout.launch_angle_rad, "rad"),
        (
            "launch_direction",
            launch.readout.launch_direction_rad,
            "rad",
        ),
        ("spin_rate", launch.readout.spin_rate_rad_s, "rad/s"),
        ("spin_axis", launch.readout.spin_axis_rad, "rad"),
        ("smash_factor", launch.smash_factor, "1"),
    ] {
        probe.value(name, value, unit, Source::Computed);
    }
    trace_vector(probe, "position", launch.position, "m");
    trace_vector(probe, "velocity", launch.velocity, "m/s");
    trace_vector(probe, "spin", launch.spin, "rad/s");
}

fn trace_vector(probe: &mut impl Probe, name: &str, vector: Vec3, unit: &str) {
    for (axis, value) in [("x", vector.x), ("y", vector.y), ("z", vector.z)] {
        probe.value(&format!("{name}_{axis}"), value, unit, Source::Computed);
    }
}
