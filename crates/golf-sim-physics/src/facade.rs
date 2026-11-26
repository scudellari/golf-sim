//! Entry points for a strike, stroke or drop with caller-supplied world samplers.

use golf_sim_base::{Real, Vec3, trace::Probe};

use crate::{
    air, impact,
    integrator::{self, Options},
    samplers::{AirSample, AirSampler, GroundSampler},
    tunables,
    values::{BallSpec, Contact, Cup, EndReason, Launch, ShotRequest, ShotResult, xz},
};

pub use crate::{
    air::{
        density as air_density, isa_pressure_pa as pressure_at_altitude, wind_at as wind_at_height,
    },
    impact::sole_height_for_strike,
    tunables::{standard_ball, standard_club, standard_club_ids},
};

/// Still air at 15 °C, 101,325 Pa and 50% relative humidity.
pub fn standard_air() -> AirSample {
    AirSample {
        temperature_c: tunables::STANDARD_TEMPERATURE_C,
        pressure_pa: tunables::ISA_P0,
        relative_humidity: tunables::STANDARD_HUMIDITY,
        ..AirSample::default()
    }
}

/// Resolve the launch from a delivery, sampling the air at impact.
pub fn strike(
    request: &ShotRequest,
    conditions: &dyn AirSampler,
    ground: &dyn GroundSampler,
    probe: &mut impl Probe,
) -> Launch {
    if !valid_request(request) {
        return invalid_launch(request.position);
    }
    let air = conditions.sample_air(request.time_s, request.position);
    if !air::valid(&air) {
        return invalid_launch(request.position);
    }
    impact::strike(request, &air, ground, probe)
}

/// Play a full stroke from delivery to rest. Events are kept when playback is off.
pub fn simulate_shot(
    request: &ShotRequest,
    conditions: &dyn AirSampler,
    ground: &dyn GroundSampler,
    playback: bool,
    probe: &mut impl Probe,
) -> ShotResult {
    let launch = strike(request, conditions, ground, probe);
    simulate_launch(
        &launch,
        &request.ball,
        request.time_s,
        conditions,
        ground,
        request.cup,
        playback,
        probe,
    )
}

/// Play a full stroke from known launch conditions.
#[expect(
    clippy::too_many_arguments,
    reason = "a launch has no shot request to carry its context"
)]
pub fn simulate_launch(
    launch: &Launch,
    ball: &BallSpec,
    time_s: Real,
    conditions: &dyn AirSampler,
    ground: &dyn GroundSampler,
    cup: Option<Cup>,
    playback: bool,
    probe: &mut impl Probe,
) -> ShotResult {
    if !valid_flight(launch, ball, time_s) || !valid_aim(launch.aim) {
        return invalid_shot(launch.position);
    }
    integrator::run(
        launch,
        ball,
        conditions,
        ground,
        cup,
        Options {
            time_s,
            playback,
            ..Options::default()
        },
        probe,
    )
}

/// Release a ball at rest. The integrator reports an invalid release if it starts in the ground.
pub fn simulate_drop(
    ball: &BallSpec,
    release_point: Vec3,
    time_s: Real,
    conditions: &dyn AirSampler,
    ground: &dyn GroundSampler,
    playback: bool,
    probe: &mut impl Probe,
) -> ShotResult {
    if !valid_ball(ball) || !time_s.is_finite() {
        return invalid_shot(release_point);
    }
    let launch = Launch {
        position: release_point,
        ..Launch::default()
    };
    integrator::run(
        &launch,
        ball,
        conditions,
        ground,
        None,
        Options {
            time_s,
            playback,
            release: true,
            ..Options::default()
        },
        probe,
    )
}

/// Horizontal carry to the given level on descent. Returns NaN if there is no airborne crossing.
/// Ground provides the starting lie and the wind's height reference.
pub fn carry_to_height(
    launch: &Launch,
    ball: &BallSpec,
    time_s: Real,
    conditions: &dyn AirSampler,
    ground: &dyn GroundSampler,
    height_m: Real,
) -> Real {
    if !valid_flight(launch, ball, time_s) || !height_m.is_finite() {
        return Real::NAN;
    }
    integrator::carry_to_height(launch, ball, time_s, conditions, ground, height_m)
}

fn valid_aim(aim: Vec3) -> bool {
    let horizontal_squared = xz(aim).length_squared();
    aim.is_finite() && horizontal_squared.is_finite() && horizontal_squared > 0.0
}

fn valid_ball(ball: &BallSpec) -> bool {
    [ball.mass_kg, ball.radius_m, ball.moment_of_inertia()]
        .into_iter()
        .all(|value| value.is_finite() && value > 0.0)
}

fn valid_flight(launch: &Launch, ball: &BallSpec, time_s: Real) -> bool {
    launch.valid
        && launch.position.is_finite()
        && launch.velocity.is_finite()
        && launch.spin.is_finite()
        && valid_ball(ball)
        && time_s.is_finite()
}

fn valid_request(request: &ShotRequest) -> bool {
    let club = &request.club;
    let delivery = &request.delivery;
    let lie = &request.lie;
    valid_ball(&request.ball)
        && valid_aim(request.aim)
        && request.position.is_finite()
        && request.time_s.is_finite()
        && request.ball_temperature_c.is_none_or(Real::is_finite)
        && [
            club.head_mass_kg,
            club.arc_radius_m,
            club.moi_forward,
            club.moi_vertical,
            club.moi_heel_toe,
        ]
        .into_iter()
        .all(|value| value.is_finite() && value > 0.0)
        && [
            delivery.club_speed_mps,
            delivery.attack_rad,
            delivery.path_rad,
            delivery.dynamic_loft_rad,
            delivery.dynamic_lie_rad,
            delivery.face_rad,
            delivery.sole_height_m,
            delivery.strike_toe_m,
            lie.ball_height_m,
            lie.grass_height_m,
            lie.grass_density,
            lie.sand_firmness,
        ]
        .into_iter()
        .all(Real::is_finite)
        && delivery.club_speed_mps >= 0.0
}

fn invalid_launch(position: Vec3) -> Launch {
    Launch {
        position: if position.is_finite() {
            position
        } else {
            Vec3::ZERO
        },
        valid: false,
        contact: Contact::Miss,
        ..Launch::default()
    }
}

fn invalid_shot(position: Vec3) -> ShotResult {
    let launch = invalid_launch(position);
    ShotResult {
        launch: Some(launch),
        end_reason: EndReason::InvalidInput,
        rest_position: launch.position,
        ..ShotResult::default()
    }
}
