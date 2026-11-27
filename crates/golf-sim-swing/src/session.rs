//! Judge the meter from caller-supplied stamps in microseconds from one clock.

use golf_sim_base::{
    Real, Seed, Vec2,
    num::{clampf, roundi},
};

use crate::{
    execution::Execution,
    noise::Drift,
    spec::{StrikeMode, SwingSpec},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Backswing,
    Downswing,
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwingView {
    pub phase: Phase,
    pub target_power: Real,
    pub power_cap: Real,
    pub meter: Real,
    pub ring_s: Real,
    pub ring_min_s: Real,
    pub ring_max_s: Real,
    pub window_early_s: Real,
    pub window_late_s: Real,
    pub timeout_after_s: Real,
    pub since_power_s: Real,
    pub since_impact_s: Real,
    /// The stroke-aware caller fills this from the meter value.
    pub club_speed_mps: Real,
    pub has_reticle: bool,
    pub reticle_m: Vec2,
    pub timing_error_s: Real,
    pub timed_out: bool,
}

/// A late-arriving input stamped before a cap or timeout can revise the judgment.
/// Hand over all inputs through the relevant stamp before acting on an execution.
#[derive(Debug)]
pub struct SwingSession {
    spec: SwingSpec,
    start_usec: i64,
    heard_usec: i64,
    presses: Vec<i64>,
    steers: Vec<(i64, Vec2)>,
    drift: Drift,
}

#[derive(Clone, Copy)]
struct PowerPress {
    usec: i64,
    meter: Real,
    ring_s: Real,
}

impl PowerPress {
    fn close_usec(self) -> i64 {
        self.usec + micros(self.ring_s)
    }
}

impl SwingSession {
    pub fn new(spec: SwingSpec, start_usec: i64, reticle_seed: Seed) -> Self {
        Self {
            spec,
            start_usec,
            heard_usec: start_usec,
            presses: Vec::new(),
            steers: Vec::new(),
            drift: Drift::new(&spec, reticle_seed),
        }
    }

    pub fn press(&mut self, time_usec: i64) {
        let index = self.presses.partition_point(|&stamp| stamp <= time_usec);
        self.presses.insert(index, time_usec);
        self.advance(time_usec);
    }

    pub fn steer(&mut self, time_usec: i64, delta_m: Vec2) {
        if self.spec.strike_mode == StrikeMode::Reticle {
            let y = if self.spec.steer_vertical {
                delta_m.y
            } else {
                0.0
            };
            self.steers.push((time_usec, Vec2::new(delta_m.x, y)));
        }
        self.advance(time_usec);
    }

    pub fn advance(&mut self, time_usec: i64) {
        self.heard_usec = self.heard_usec.max(time_usec);
    }

    pub fn is_done(&self) -> bool {
        self.power_press()
            .is_some_and(|(power, impact)| self.impact(power, impact).is_some())
    }

    pub fn execution(&self) -> Option<Execution> {
        let (power, impact) = self.power_press()?;
        self.judge(power, impact)
    }

    /// The instant the ring closes. Zero while the meter is still rising.
    pub fn close_usec(&self) -> i64 {
        self.power_press()
            .map_or(0, |(power, _)| power.close_usec())
    }

    /// Read the current swing with display values at the supplied stamp.
    pub fn view_at(&self, time_usec: i64) -> SwingView {
        let spec = &self.spec;
        let mut view = SwingView {
            phase: Phase::Backswing,
            target_power: spec.target_power,
            power_cap: spec.power_cap,
            meter: self.meter_at(time_usec),
            ring_s: spec.ring_for_backswing(seconds(time_usec - self.start_usec)),
            ring_min_s: spec.ring_min_s,
            ring_max_s: spec.ring_max_s,
            window_early_s: spec.window_early_s,
            window_late_s: spec.window_late_s,
            timeout_after_s: spec.timeout_after_s,
            since_power_s: 0.0,
            since_impact_s: 0.0,
            club_speed_mps: 0.0,
            has_reticle: spec.strike_mode == StrikeMode::Reticle,
            reticle_m: Vec2::ZERO,
            timing_error_s: 0.0,
            timed_out: false,
        };
        if let Some((power, impact)) = self.power_press() {
            view.meter = power.meter;
            view.ring_s = power.ring_s;
            if let Some(execution) = self.judge(power, impact) {
                view.phase = Phase::Done;
                view.since_power_s = seconds(execution.impact_usec - power.usec);
                view.since_impact_s = seconds(time_usec - execution.impact_usec).max(0.0);
                view.reticle_m = execution.strike_m.unwrap_or(Vec2::ZERO);
                view.timing_error_s = execution.timing_error_s;
                view.timed_out = execution.timed_out;
            } else {
                view.phase = Phase::Downswing;
                view.since_power_s = seconds(time_usec - power.usec);
                view.reticle_m = self.reticle(time_usec, Some(power)).unwrap_or(Vec2::ZERO);
            }
        } else {
            view.reticle_m = self.reticle(time_usec, None).unwrap_or(Vec2::ZERO);
        }
        view
    }

    pub fn reticle_at(&self, time_usec: i64) -> Vec2 {
        let power = self.power_press().map(|(power, _)| power);
        self.reticle(time_usec, power).unwrap_or(Vec2::ZERO)
    }

    fn meter_at(&self, time_usec: i64) -> Real {
        let elapsed_s = seconds(time_usec - self.start_usec);
        clampf(
            elapsed_s * self.spec.meter_rate_per_s,
            0.0,
            self.spec.power_cap,
        )
    }

    fn power_press(&self) -> Option<(PowerPress, Option<i64>)> {
        let cap_usec = self.start_usec + micros(self.spec.power_cap / self.spec.meter_rate_per_s);
        let (usec, meter, next_press) = match self.presses.as_slice() {
            [first, rest @ ..] if *first < cap_usec => {
                (*first, self.meter_at(*first), rest.first().copied())
            }
            presses if self.heard_usec >= cap_usec => {
                (cap_usec, self.spec.power_cap, presses.first().copied())
            }
            _ => return None,
        };
        let backswing_s = seconds(usec - self.start_usec);
        let ring_s = self.spec.ring_for_backswing(backswing_s);
        Some((
            PowerPress {
                usec,
                meter,
                ring_s,
            },
            next_press,
        ))
    }

    fn impact(&self, power: PowerPress, press: Option<i64>) -> Option<(i64, bool)> {
        let timeout_usec = power.close_usec() + micros(self.spec.timeout_after_s);
        if let Some(press) = press.filter(|&stamp| stamp < timeout_usec) {
            Some((press, false))
        } else {
            (self.heard_usec >= timeout_usec).then_some((timeout_usec, true))
        }
    }

    fn judge(&self, power: PowerPress, press: Option<i64>) -> Option<Execution> {
        let (impact_usec, timed_out) = self.impact(power, press)?;
        let timing_error_s = seconds(impact_usec - power.close_usec());
        let window_s = if timing_error_s > 0.0 {
            self.spec.window_late_s
        } else {
            self.spec.window_early_s
        };
        Some(Execution {
            power: power.meter,
            timing_error_s,
            timing_fault: timing_error_s / window_s,
            timed_out,
            strike_m: self.reticle(impact_usec, Some(power)),
            impact_usec,
            backswing_s: seconds(power.usec - self.start_usec),
            ring_s: power.ring_s,
        })
    }

    fn reticle(&self, time_usec: i64, power: Option<PowerPress>) -> Option<Vec2> {
        if self.spec.strike_mode == StrikeMode::None {
            return None;
        }
        let elapsed_s = seconds(time_usec - self.start_usec).max(0.0);
        let mut slide_s = elapsed_s;
        if let Some(power) = power.filter(|power| time_usec > power.usec) {
            let backswing_s = seconds(power.usec - self.start_usec);
            let into_s =
                seconds(time_usec - power.usec).min(power.ring_s + self.spec.timeout_after_s);
            slide_s = backswing_s
                + into_s
                + (self.spec.downswing_slide_gain - 1.0) * into_s * into_s / (2.0 * power.ring_s);
        }
        let wobble = self.drift.wobble(self.spec.wobble_m, elapsed_s);
        let slide_m = self.drift.slide_mps * slide_s;
        let steering = self
            .steers
            .iter()
            .filter(|&&(stamp, _)| stamp <= time_usec)
            .fold(Vec2::ZERO, |sum, &(_, delta)| {
                Vec2::new(sum.x + delta.x, sum.y + delta.y)
            });
        Some(Vec2::new(
            self.drift.start_m.x + slide_m.x + wobble.x + steering.x,
            self.drift.start_m.y + slide_m.y + wobble.y + steering.y,
        ))
    }
}

fn seconds(usec: i64) -> Real {
    usec as Real / 1_000_000.0
}

fn micros(seconds: Real) -> i64 {
    roundi(seconds * 1_000_000.0)
}
