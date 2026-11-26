//! Ground skid and rolling resistance on a sampled surface.

use golf_sim_base::{
    Real, Vec3, fm, log,
    num::clampf,
    trace::{Phase, Probe, Source},
};

use crate::{
    samplers::GroundSample,
    tunables as t,
    values::{BallSpec, UP},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Motion {
    #[default]
    Skid,
    Roll,
    Rest,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct State {
    pub velocity: Vec3,
    pub spin: Vec3,
    pub motion: Motion,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Air {
    pub density: Real,
    pub viscosity: Real,
    pub half_area_per_mass: Real,
}

/// Rolling deceleration on level ground, in metres per second squared.
pub fn resistance(ground: &GroundSample, speed: Real) -> Real {
    let props = t::surface(ground.surface);
    let exit_speed_squared = t::STIMP_EXIT_SPEED * t::STIMP_EXIT_SPEED;
    if ground.green_speed_m > 0.0 && props.uses_green_speed {
        return exit_speed_squared / (2.0 * ground.green_speed_m);
    }

    let moisture = clampf(ground.moisture - t::MOISTURE_REFERENCE, -1.0, 1.0);
    let firmness = clampf(ground.firmness, 0.0, 1.0);
    let mut stimp = props.stimp_m / (1.0 + t::ROLL_MOISTURE_SLOWDOWN * moisture);
    stimp *= 1.0 + t::ROLL_FIRMNESS_GAIN * (firmness - 0.5);
    exit_speed_squared / (2.0 * stimp) + props.grass_drag * speed
}

/// A stimp reading already includes air drag up to its exit speed.
pub fn air_drag(velocity: Vec3, spin: Vec3, air: &Air, ball: &BallSpec) -> Real {
    let speed_squared = velocity.length_squared();
    let excess = speed_squared - t::STIMP_EXIT_SPEED * t::STIMP_EXIT_SPEED;
    if excess <= 0.0 {
        return 0.0;
    }

    let reynolds = air.density * fm::sqrt(speed_squared) * 2.0 * ball.radius_m / air.viscosity;
    let spin_ratio = ball.radius_m * spin.cross(velocity).length() / speed_squared;
    let ratio = clampf(spin_ratio, 0.0, t::CD_SPIN_PEAK);
    let crisis = clampf((t::CD_CRISIS_RE - reynolds) / t::CD_CRISIS_WIDTH, 0.0, 1.0);
    let drag =
        t::CD_BASE + t::CD_SPIN * ratio + t::CD_SPIN2 * ratio * ratio + t::CD_CRISIS_RISE * crisis;
    air.half_area_per_mass * air.density * drag * excess
}

pub fn gravity_along(normal: Vec3) -> Vec3 {
    (-UP * t::GRAVITY).slide(normal)
}

/// Advances the contact velocity and spin. The caller keeps the ball on the surface.
pub fn step<P: Probe>(
    state: &mut State,
    ground: &GroundSample,
    ball: &BallSpec,
    air: &Air,
    dt: Real,
    probe: &mut P,
) {
    let skidding = state.motion == Motion::Skid;
    let _span = if skidding { log::skid() } else { log::roll() }.entered();
    let normal = ground.normal;
    let k = ball.inertia_factor;
    state.velocity = state.velocity.slide(normal);
    let downhill = gravity_along(normal);
    let speed = state.velocity.length();
    let drag = air_drag(state.velocity, state.spin, air, ball);

    if skidding {
        let contact = -normal * ball.radius_m;
        let slip = (state.velocity + state.spin.cross(contact)).slide(normal);
        let friction = t::surface(ground.surface).skid_friction;
        let normal_accel = t::GRAVITY * normal.y;
        let limit = friction * normal_accel * dt;
        let to_roll = slip * (-k / (1.0 + k));
        let change = if to_roll.length() <= limit {
            state.motion = Motion::Roll;
            to_roll
        } else {
            to_roll.normalized() * limit
        };
        state.velocity += change;
        state.spin += contact.cross(change * ball.mass_kg) / ball.moment_of_inertia();
        if drag > 0.0 {
            state.velocity = state.velocity / (1.0 + drag * dt / speed);
        }
        state.velocity += downhill * dt;

        if P::ENABLED {
            probe.begin_phase(Phase::Skid);
            probe.value("slip_speed", slip.length(), "m/s", Source::Computed);
            probe.value(
                "skid_friction",
                friction,
                "",
                Source::Turf {
                    name: "skid_friction".into(),
                },
            );
            probe.value("friction_limit", limit, "m/s", Source::Computed);
            probe.value(
                "gravity",
                t::GRAVITY,
                "m/s²",
                Source::Tunable {
                    name: "GRAVITY".into(),
                },
            );
            if state.motion == Motion::Roll {
                probe.begin_phase(Phase::Roll);
                probe.value(
                    "rolling_started",
                    state.velocity.length(),
                    "m/s",
                    Source::Computed,
                );
            }
        }
        return;
    }

    let drive = downhill / (1.0 + k);
    let rolling_decel = resistance(ground, speed);
    let decel = rolling_decel + drag / (1.0 + k);
    let can_stop = drive.length() <= decel;
    if speed < t::ROLL_STOP_SPEED && can_stop {
        stop(
            state,
            ground,
            speed,
            rolling_decel,
            "stopped_below_speed_threshold",
            probe,
        );
        return;
    }

    let mut velocity = state.velocity + drive * dt;
    let speed_with_drive = velocity.length();
    if speed_with_drive > 0.0 {
        let slowed = (speed_with_drive - decel * dt).max(0.0);
        if slowed == 0.0 && can_stop {
            stop(
                state,
                ground,
                speed,
                rolling_decel,
                "stopped_by_resistance",
                probe,
            );
            return;
        }
        velocity = velocity * (slowed / speed_with_drive);
    }
    state.velocity = velocity;
    state.spin = normal.cross(velocity) / ball.radius_m;
}

fn stop<P: Probe>(
    state: &mut State,
    ground: &GroundSample,
    speed: Real,
    resistance: Real,
    reason: &str,
    probe: &mut P,
) {
    state.velocity = Vec3::ZERO;
    state.spin = Vec3::ZERO;
    state.motion = Motion::Rest;
    if P::ENABLED {
        probe.begin_phase(Phase::Roll);
        probe.value(reason, speed, "m/s", Source::Computed);
        probe.value(
            "stop_speed",
            t::ROLL_STOP_SPEED,
            "m/s",
            Source::Tunable {
                name: "ROLL_STOP_SPEED".into(),
            },
        );
        probe.value("resistance", resistance, "m/s²", Source::Computed);
        probe.value(
            "stimp_exit_speed",
            t::STIMP_EXIT_SPEED,
            "m/s",
            Source::Tunable {
                name: "STIMP_EXIT_SPEED".into(),
            },
        );
        let props = t::surface(ground.surface);
        if ground.green_speed_m > 0.0 && props.uses_green_speed {
            probe.value(
                "green_speed",
                ground.green_speed_m,
                "m",
                Source::Turf {
                    name: "green_speed_m".into(),
                },
            );
        } else {
            for (name, value, unit) in [
                ("stimp_m", props.stimp_m, "m"),
                ("firmness", ground.firmness, ""),
                ("moisture", ground.moisture, ""),
                ("grass_drag", props.grass_drag, "1/s"),
            ] {
                probe.value(name, value, unit, Source::Turf { name: name.into() });
            }
            for (name, value) in [
                ("ROLL_MOISTURE_SLOWDOWN", t::ROLL_MOISTURE_SLOWDOWN),
                ("MOISTURE_REFERENCE", t::MOISTURE_REFERENCE),
                ("ROLL_FIRMNESS_GAIN", t::ROLL_FIRMNESS_GAIN),
            ] {
                probe.value(name, value, "", Source::Tunable { name: name.into() });
            }
        }
    }
}
