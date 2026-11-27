//! Exact meter results and seeded hands that press the meter.

use golf_sim_base::{Real, Seed, Vec2, fm, num::roundi, seed::SeedStream};

use crate::{
    execution::Execution,
    session::SwingSession,
    spec::{StrikeMode, SwingSpec},
};

const USEC_PER_S: Real = 1_000_000.0;
const USEC_PER_MS: Real = 1_000.0;
const STEER_STEP_USEC: i64 = 10_000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Swing {
    Exact {
        power: Real,
        timing_fault: Real,
        strike_m: Option<Vec2>,
    },
    Skill(Profile),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Profile {
    pub name: &'static str,
    pub sigma_p_ms: Real,
    pub bias_t_ms: Real,
    pub sigma_t_ms: Real,
    pub sigma_s_mm: Real,
    pub tau_ms: Real,
}

impl Profile {
    // These values await calibration against measured shot dispersion.
    pub const SCRATCH: Self = Self {
        name: "scratch",
        sigma_p_ms: 12.0,
        bias_t_ms: 0.0,
        sigma_t_ms: 6.0,
        sigma_s_mm: 2.0,
        tau_ms: 50.0,
    };

    pub const HCP15: Self = Self {
        name: "hcp15",
        sigma_p_ms: 30.0,
        bias_t_ms: 0.0,
        sigma_t_ms: 15.0,
        sigma_s_mm: 5.0,
        tau_ms: 80.0,
    };
}

impl Swing {
    /// Resolve a skill swing at the spec's target power, aiming at the centre of the face.
    pub fn execute(self, spec: &SwingSpec, start_usec: i64, attempt_seed: Seed) -> Execution {
        match self {
            Self::Exact { .. } => exact(spec, self, start_usec),
            Self::Skill(profile) => skill(spec, profile, Vec2::ZERO, start_usec, attempt_seed),
        }
    }
}

/// Fill in the clock values for a stated meter result.
///
/// Panics if `swing` is a skill profile.
pub fn exact(spec: &SwingSpec, swing: Swing, start_usec: i64) -> Execution {
    let Swing::Exact {
        power,
        timing_fault,
        strike_m,
    } = swing
    else {
        panic!("exact requires Swing::Exact");
    };
    let backswing_s = power / spec.meter_rate_per_s;
    let ring_s = spec.ring_for_backswing(backswing_s);
    let window_s = if timing_fault > 0.0 {
        spec.window_late_s
    } else {
        spec.window_early_s
    };
    let timing_error_s = timing_fault * window_s;
    // The meter stamps the power press and the ring's close separately.
    let impact_usec = start_usec + micros(backswing_s) + micros(ring_s) + micros(timing_error_s);
    Execution {
        power,
        timing_error_s,
        timing_fault,
        timed_out: false,
        strike_m: strike_m.filter(|_| spec.strike_mode == StrikeMode::Reticle),
        impact_usec,
        backswing_s,
        ring_s,
    }
}

/// Press and steer a session at the spec's target power and the wanted strike point.
pub fn skill(
    spec: &SwingSpec,
    profile: Profile,
    wanted_strike_m: Vec2,
    start_usec: i64,
    attempt_seed: Seed,
) -> Execution {
    let mut hands = SeedStream::derived(attempt_seed, "hands");
    let power_error_ms = hands.next_gaussian() * profile.sigma_p_ms;
    let timing_error_ms = profile.bias_t_ms + hands.next_gaussian() * profile.sigma_t_ms;
    let strike_error_m =
        Vec2::new(hands.next_gaussian(), hands.next_gaussian()) * (profile.sigma_s_mm / 1_000.0);

    let mut session = SwingSession::new(*spec, start_usec, attempt_seed.derive("reticle"));
    let cap_usec = micros(spec.power_cap / spec.meter_rate_per_s);
    let wanted_power_usec = spec.target_power / spec.meter_rate_per_s * USEC_PER_S;
    let power_usec = roundi(wanted_power_usec + power_error_ms * USEC_PER_MS).clamp(0, cap_usec);
    let power_stamp = start_usec + power_usec;
    if power_usec < cap_usec {
        session.press(power_stamp);
    } else {
        // The cap has already set power by the time this hand would press.
        session.advance(power_stamp);
    }

    let close_usec = session.close_usec();
    let impact_usec = close_usec
        .saturating_add(roundi(timing_error_ms * USEC_PER_MS))
        .max(power_stamp);
    if spec.strike_mode == StrikeMode::Reticle {
        let until_usec = impact_usec.min(close_usec + micros(spec.timeout_after_s));
        let target = Vec2::new(
            wanted_strike_m.x + strike_error_m.x,
            wanted_strike_m.y + strike_error_m.y,
        );
        let step_ms = STEER_STEP_USEC as Real / USEC_PER_MS;
        let gain = -fm::expm1(-step_ms / profile.tau_ms);
        let mut stamp = start_usec + STEER_STEP_USEC;
        while stamp <= until_usec {
            let delta = (target - session.reticle_at(stamp)) * gain;
            session.steer(stamp, delta);
            stamp += STEER_STEP_USEC;
        }
    }
    session.press(impact_usec);
    session.execution().expect("both swing presses resolved")
}

fn micros(seconds: Real) -> i64 {
    roundi(seconds * USEC_PER_S)
}
