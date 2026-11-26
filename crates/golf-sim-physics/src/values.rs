//! Inputs and outputs of the physics model.

use golf_sim_base::{Real, Vec2, Vec3, fm, num::clampf};

pub use golf_sim_base::Surface;

pub(crate) const UP: Vec3 = Vec3::new(0.0, 1.0, 0.0);
const FORWARD: Vec3 = Vec3::new(0.0, 0.0, -1.0);

#[derive(Clone, Debug, PartialEq)]
pub struct ShotRequest {
    pub club: ClubSpec,
    pub ball: BallSpec,
    pub delivery: Delivery,
    pub lie: Lie,
    /// Soil surface under the ball.
    pub position: Vec3,
    /// Horizontal aim direction.
    pub aim: Vec3,
    /// Round clock at impact.
    pub time_s: Real,
    /// Uses the local air temperature when absent.
    pub ball_temperature_c: Option<Real>,
    pub cup: Option<Cup>,
}

impl ShotRequest {
    pub fn new(club: ClubSpec, ball: BallSpec, delivery: Delivery, lie: Lie) -> Self {
        Self {
            club,
            ball,
            delivery,
            lie,
            position: Vec3::ZERO,
            aim: FORWARD,
            time_s: 0.0,
            ball_temperature_c: None,
            cup: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShotResult {
    pub launch: Option<Launch>,
    /// Sample times since impact, in increasing order and paired with `positions`.
    pub times: Vec<Real>,
    /// World-space ball centres for playback.
    pub positions: Vec<Vec3>,
    pub events: Vec<ShotEvent>,
    pub touchdowns: Vec<Touchdown>,
    pub end_reason: EndReason,
    /// Where the next stroke is played. On timeout this can be below the last sample.
    pub rest_position: Vec3,
    pub rest_surface: Surface,
    pub embedded: bool,
}

impl Default for ShotResult {
    fn default() -> Self {
        Self {
            launch: None,
            times: Vec::new(),
            positions: Vec::new(),
            events: Vec::new(),
            touchdowns: Vec::new(),
            end_reason: EndReason::Rest,
            rest_position: Vec3::ZERO,
            rest_surface: Surface::Fairway,
            embedded: false,
        }
    }
}

impl ShotResult {
    pub fn position_at(&self, time_s: Real) -> Vec3 {
        let Some(last) = self.times.len().checked_sub(1) else {
            return self.rest_position;
        };
        if last == 0 || time_s <= self.times[0] {
            return self.positions[0];
        }
        if time_s >= self.times[last] {
            return self.positions[last];
        }
        let hi = self.times.partition_point(|&time| time <= time_s).max(1);
        let lo = hi - 1;
        let weight = (time_s - self.times[lo]) / (self.times[hi] - self.times[lo]);
        self.positions[lo].lerp(self.positions[hi], weight)
    }

    pub fn last_zone_entry(&self, zone: Zone) -> Option<&ShotEvent> {
        self.events
            .iter()
            .rfind(|event| event.kind == EventKind::ZoneEntered && event.zone == zone)
    }

    pub fn first_event(&self, kind: EventKind) -> Option<&ShotEvent> {
        self.events.iter().find(|event| event.kind == kind)
    }

    pub fn count_events(&self, kind: EventKind) -> usize {
        self.events
            .iter()
            .filter(|event| event.kind == kind)
            .count()
    }

    fn landing_or_start(&self, launch: &Launch) -> Vec3 {
        self.touchdowns
            .first()
            .map_or(launch.position, |touchdown| touchdown.position)
    }

    pub fn carry_m(&self) -> Real {
        self.launch.as_ref().map_or(0.0, |launch| {
            xz(self.landing_or_start(launch) - launch.position).length()
        })
    }

    pub fn carry_side_m(&self) -> Real {
        self.launch.as_ref().map_or(0.0, |launch| {
            xz(self.landing_or_start(launch) - launch.position).dot(xz(launch.aim.cross(UP)))
        })
    }

    pub fn total_m(&self) -> Real {
        self.launch.as_ref().map_or(0.0, |launch| {
            xz(self.rest_position - launch.position).length()
        })
    }

    pub fn total_side_m(&self) -> Real {
        self.launch.as_ref().map_or(0.0, |launch| {
            xz(self.rest_position - launch.position).dot(xz(launch.aim.cross(UP)))
        })
    }

    pub fn total_along_m(&self) -> Real {
        self.launch.as_ref().map_or(0.0, |launch| {
            xz(self.rest_position - launch.position).dot(xz(launch.aim))
        })
    }

    pub fn run_m(&self) -> Real {
        self.launch.as_ref().map_or(0.0, |launch| {
            self.total_along_m()
                - xz(self.landing_or_start(launch) - launch.position).dot(xz(launch.aim))
        })
    }

    pub fn apex_height_m(&self) -> Real {
        match (&self.launch, self.first_event(EventKind::Apex)) {
            (Some(launch), Some(apex)) => apex.position.y - launch.position.y,
            _ => 0.0,
        }
    }

    pub fn hang_time_s(&self) -> Real {
        self.touchdowns
            .first()
            .map_or(0.0, |touchdown| touchdown.time_s)
    }

    pub fn land_angle_rad(&self) -> Real {
        self.touchdowns.first().map_or(0.0, |touchdown| {
            fm::atan2(-touchdown.velocity.y, xz(touchdown.velocity).length())
        })
    }
}

pub(crate) fn xz(position: Vec3) -> Vec2 {
    Vec2::new(position.x, position.z)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum EndReason {
    Rest,
    Holed,
    Water,
    Timeout,
    InvalidRelease,
    InvalidInput,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShotEvent {
    pub kind: EventKind,
    pub time_s: Real,
    pub position: Vec3,
    pub velocity: Vec3,
    pub spin: Vec3,
    pub surface: Surface,
    pub zone: Zone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum EventKind {
    Impact,
    Apex,
    Landing,
    Bounce,
    RollStart,
    PureRoll,
    Obstacle,
    Canopy,
    SurfaceEntered,
    ZoneEntered,
    LipOut,
    Holed,
    Rest,
    Release,
    CupEntry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Zone {
    #[default]
    InPlay,
    PenaltyArea,
    OutOfBounds,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Touchdown {
    pub time_s: Real,
    pub position: Vec3,
    pub contact_point: Vec3,
    /// Incoming velocity, before the ground responds.
    pub velocity: Vec3,
    /// Pitch-mark depth.
    pub depth_m: Real,
    pub surface: Surface,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LaunchReadout {
    pub ball_speed_mps: Real,
    pub launch_angle_rad: Real,
    pub launch_direction_rad: Real,
    pub spin_rate_rad_s: Real,
    pub spin_axis_rad: Real,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Launch {
    pub position: Vec3,
    pub velocity: Vec3,
    /// World-space spin vector (rad/s, right-hand rule).
    pub spin: Vec3,
    pub aim: Vec3,
    pub readout: LaunchReadout,
    pub club_speed_mps: Real,
    pub club_speed_at_ball_mps: Real,
    pub smash_factor: Real,
    pub club_path_rad: Real,
    pub attack_angle_rad: Real,
    pub face_angle_rad: Real,
    pub dynamic_loft_rad: Real,
    pub face_to_path_rad: Real,
    pub spin_loft_rad: Real,
    pub valid: bool,
    pub contact: Contact,
    pub strike_toe_m: Real,
    pub strike_up_m: Real,
    pub low_point_ahead_m: Real,
    pub low_point_height_m: Real,
    pub turf_entry_behind_m: Real,
    pub turf_contact_start: Option<Vec3>,
    pub turf_contact_end: Option<Vec3>,
    pub turf_contact_depth_m: Real,
    pub interference: Real,
    pub ball_temperature_c: Real,
}

impl Default for Launch {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            velocity: Vec3::ZERO,
            spin: Vec3::ZERO,
            aim: FORWARD,
            readout: LaunchReadout::default(),
            club_speed_mps: 0.0,
            club_speed_at_ball_mps: 0.0,
            smash_factor: 0.0,
            club_path_rad: 0.0,
            attack_angle_rad: 0.0,
            face_angle_rad: 0.0,
            dynamic_loft_rad: 0.0,
            face_to_path_rad: 0.0,
            spin_loft_rad: 0.0,
            valid: true,
            contact: Contact::Face,
            strike_toe_m: 0.0,
            strike_up_m: 0.0,
            low_point_ahead_m: 0.0,
            low_point_height_m: 0.0,
            turf_entry_behind_m: 0.0,
            turf_contact_start: None,
            turf_contact_end: None,
            turf_contact_depth_m: 0.0,
            interference: 0.0,
            ball_temperature_c: 0.0,
        }
    }
}

impl Launch {
    pub fn from_readout(
        ball: &BallSpec,
        ground_point: Vec3,
        aim: Vec3,
        readout: LaunchReadout,
    ) -> Self {
        let aim = Vec3::new(aim.x, 0.0, aim.z).normalized();
        let right = aim.cross(UP);
        let horizontal = aim * fm::cos(readout.launch_direction_rad)
            + right * fm::sin(readout.launch_direction_rad);
        let direction =
            horizontal * fm::cos(readout.launch_angle_rad) + UP * fm::sin(readout.launch_angle_rad);
        let side = horizontal.cross(UP);
        let spin_axis = side * fm::cos(readout.spin_axis_rad) - UP * fm::sin(readout.spin_axis_rad);
        Self {
            position: ground_point + UP * ball.radius_m,
            velocity: direction * readout.ball_speed_mps,
            spin: spin_axis * readout.spin_rate_rad_s,
            aim,
            readout,
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Contact {
    Face,
    LeadingEdge,
    TopEdge,
    Sand,
    SideEdge,
    Miss,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Delivery {
    pub club_speed_mps: Real,
    /// In-to-out is positive.
    pub path_rad: Real,
    /// Ascending is positive.
    pub attack_rad: Real,
    /// Open is positive.
    pub face_rad: Real,
    pub dynamic_loft_rad: Real,
    /// Toe up is positive.
    pub dynamic_lie_rad: Real,
    pub strike_toe_m: Real,
    /// Leading-edge height above the soil under the ball.
    pub sole_height_m: Real,
}

impl Delivery {
    /// Club path for a head moving on a swing plane as it meets the ball.
    pub fn path_on_plane(swing_direction_rad: Real, plane_rad: Real, attack_rad: Real) -> Real {
        let along = fm::asin(clampf(fm::sin(attack_rad) / fm::sin(plane_rad), -1.0, 1.0));
        swing_direction_rad + fm::atan2(-fm::cos(plane_rad) * fm::sin(along), fm::cos(along))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum ClubCategory {
    Driver,
    Wood,
    Hybrid,
    #[default]
    Iron,
    Wedge,
    Putter,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClubSpec {
    pub id: String,
    pub category: ClubCategory,
    pub loft_rad: Real,
    pub length_m: Real,
    pub head_mass_kg: Real,
    pub cor: Real,
    pub cor_speed_slope_delta: Real,
    pub cor_falloff_toe: Real,
    pub cor_falloff_up: Real,
    pub cg_depth_m: Real,
    pub cg_up_m: Real,
    pub cg_toe_m: Real,
    pub moi_vertical: Real,
    pub moi_heel_toe: Real,
    pub moi_forward: Real,
    pub bulge_radius_m: Real,
    pub roll_radius_m: Real,
    pub face_center_height_m: Real,
    pub face_height_m: Real,
    pub face_half_width_m: Real,
    pub face_friction: Real,
    pub tangential_restitution_up: Real,
    pub tangential_restitution_toe: Real,
    pub tangential_restitution_speed_slope: Real,
    pub arc_radius_m: Real,
    pub sole_width_m: Real,
    pub sole_bounce_rad: Real,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BallSpec {
    pub mass_kg: Real,
    pub radius_m: Real,
    pub inertia_factor: Real,
}

impl BallSpec {
    pub fn moment_of_inertia(&self) -> Real {
        self.inertia_factor * self.mass_kg * self.radius_m * self.radius_m
    }

    pub fn area(&self) -> Real {
        std::f64::consts::PI * self.radius_m * self.radius_m
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lie {
    pub surface: Surface,
    /// Height of the bottom of the ball above the soil, or the depth of sand around it in a bunker.
    pub ball_height_m: Real,
    pub grass_height_m: Real,
    pub grass_density: Real,
    pub sand_firmness: Real,
}

impl Default for Lie {
    fn default() -> Self {
        Self {
            surface: Surface::Fairway,
            ball_height_m: 0.0,
            grass_height_m: 0.0,
            grass_density: 0.0,
            sand_firmness: 0.5,
        }
    }
}

impl Lie {
    pub fn tee(height_m: Real) -> Self {
        Self {
            surface: Surface::Tee,
            ball_height_m: height_m,
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cup {
    /// Centre of the cup at the green surface.
    pub position: Vec3,
    pub flagstick_in: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObstacleHit {
    pub kind: ObstacleKind,
    /// World position of the ball centre touching the inflated obstacle.
    pub point: Vec3,
    pub normal: Vec3,
    pub fraction: Real,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ObstacleKind {
    Trunk,
    Branch,
    Rock,
    Wall,
    BunkerFace,
    Flagstick,
    CupRim,
}

pub mod units {
    use golf_sim_base::Real;

    pub const MPH: Real = 0.44704;
    pub const YARD: Real = 0.9144;
    pub const FOOT: Real = 0.3048;
    pub const INCH: Real = 0.0254;
    pub const RPM: Real = std::f64::consts::TAU / 60.0;

    pub const fn mph(value: Real) -> Real {
        value * MPH
    }
    pub const fn to_mph(mps: Real) -> Real {
        mps / MPH
    }
    pub const fn yards(value: Real) -> Real {
        value * YARD
    }
    pub const fn to_yards(meters: Real) -> Real {
        meters / YARD
    }
    pub const fn feet(value: Real) -> Real {
        value * FOOT
    }
    pub const fn to_feet(meters: Real) -> Real {
        meters / FOOT
    }
    pub const fn inches(value: Real) -> Real {
        value * INCH
    }
    pub const fn rpm(value: Real) -> Real {
        value * RPM
    }
    pub const fn to_rpm(rad_per_s: Real) -> Real {
        rad_per_s / RPM
    }
    pub const fn fahrenheit_to_celsius(value: Real) -> Real {
        (value - 32.0) * 5.0 / 9.0
    }
}
