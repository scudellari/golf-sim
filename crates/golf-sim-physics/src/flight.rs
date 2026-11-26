//! Aerodynamic forces, flight stepping, and flight diagnostics.

use golf_sim_base::{
    Real, Vec3, fm, log,
    num::clampf,
    trace::{Phase, Probe, Source},
};

use crate::{air::FlightAir, tunables as t, values::BallSpec};

/// Drag rises below the drag crisis and with spin up to the peak of the parabola.
pub fn drag(reynolds: Real, spin_ratio: Real) -> Real {
    let spin = clampf(spin_ratio, 0.0, t::CD_SPIN_PEAK);
    let crisis = clampf((t::CD_CRISIS_RE - reynolds) / t::CD_CRISIS_WIDTH, 0.0, 1.0);
    t::CD_BASE + t::CD_SPIN * spin + t::CD_SPIN2 * spin * spin + t::CD_CRISIS_RISE * crisis
}

pub fn lift(spin_ratio: Real) -> Real {
    t::CL_SCALE * fm::pow(spin_ratio.max(0.0), t::CL_EXPONENT)
}

/// Returns (drag coefficient, lift coefficient).
pub fn coefficients(reynolds: Real, spin_ratio: Real) -> (Real, Real) {
    (drag(reynolds, spin_ratio), lift(spin_ratio))
}

pub fn acceleration(velocity: Vec3, spin: Vec3, air: &FlightAir, ball: &BallSpec) -> Vec3 {
    acceleration_with_area(
        velocity,
        spin,
        air,
        ball.radius_m,
        0.5 * ball.area() / ball.mass_kg,
    )
}

fn acceleration_with_area(
    velocity: Vec3,
    spin: Vec3,
    air: &FlightAir,
    radius_m: Real,
    half_area_per_mass: Real,
) -> Vec3 {
    let relative = velocity - air.wind_mps;
    let speed = relative.length();
    let mut acceleration = Vec3::new(0.0, -t::GRAVITY, 0.0);
    if speed < 1e-6 {
        return acceleration;
    }
    let lift_axis = spin.cross(relative);
    let cross_length = lift_axis.length();
    let spin_ratio = radius_m * cross_length / (speed * speed);
    let reynolds = air.density_kg_m3 * speed * 2.0 * radius_m / air.viscosity_pa_s;
    let (cd, cl) = coefficients(reynolds, spin_ratio);
    let force_factor = half_area_per_mass * air.density_kg_m3 * speed;
    acceleration -= relative * (force_factor * cd);
    if cross_length > 1e-9 {
        acceleration += lift_axis * (force_factor * cl * speed / cross_length);
    }
    if air.canopy_density > 0.0 {
        acceleration -= velocity * (t::CANOPY_DRAG_PER_M * air.canopy_density * velocity.length());
    }
    acceleration
}

/// Decay spin over the elapsed part of the step, using the average air-relative speed.
pub fn decay_spin(
    spin: Vec3,
    before: Vec3,
    after: Vec3,
    air: &FlightAir,
    ball: &BallSpec,
    dt: Real,
) -> Vec3 {
    let average_speed = ((before - air.wind_mps).length() + (after - air.wind_mps).length()) * 0.5;
    let rate = t::SPIN_DECAY_LAMBDA * average_speed / ball.radius_m
        + t::CANOPY_SPIN_LOSS_PER_S * air.canopy_density;
    spin * fm::exp(-rate * dt)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Step {
    pub travel: Vec3,
    pub velocity: Vec3,
}

/// One RK4 step of position and velocity, with spin and air held fixed.
pub fn step(velocity: Vec3, spin: Vec3, air: &FlightAir, ball: &BallSpec, dt: Real) -> Step {
    let half_area_per_mass = 0.5 * ball.area() / ball.mass_kg;
    let acceleration = |v| acceleration_with_area(v, spin, air, ball.radius_m, half_area_per_mass);
    let a1 = acceleration(velocity);
    let v2 = velocity + a1 * (dt * 0.5);
    let a2 = acceleration(v2);
    let v3 = velocity + a2 * (dt * 0.5);
    let a3 = acceleration(v3);
    let v4 = velocity + a3 * dt;
    let a4 = acceleration(v4);
    Step {
        travel: (velocity + v2 * 2.0 + v3 * 2.0 + v4) * (dt / 6.0),
        velocity: velocity + (a1 + a2 * 2.0 + a3 * 2.0 + a4) * (dt / 6.0),
    }
}

/// State of the ball at an instant in flight. Time is measured from impact.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlightPoint {
    pub time_s: Real,
    pub position: Vec3,
    pub velocity: Vec3,
    pub spin: Vec3,
}

impl FlightPoint {
    fn interpolate(self, to: Self, time_s: Real, fraction: Real) -> Self {
        Self {
            time_s,
            position: self.position.lerp(to.position, fraction),
            velocity: self.velocity.lerp(to.velocity, fraction),
            spin: self.spin.lerp(to.spin, fraction),
        }
    }

    /// Estimate the apex within a step where the vertical velocity crosses zero.
    pub fn apex_between(self, to: Self) -> Option<Self> {
        if self.velocity.y <= 0.0 || to.velocity.y > 0.0 {
            return None;
        }
        let fraction = self.velocity.y / (self.velocity.y - to.velocity.y);
        let elapsed = (to.time_s - self.time_s) * fraction;
        let mut apex = self.interpolate(to, self.time_s + elapsed, fraction);
        // Integrate from the starting vertical velocity to zero with linear acceleration.
        apex.position.y = self.position.y + self.velocity.y * elapsed * 0.5;
        Some(apex)
    }
}

/// Records a flight while the caller advances the solver and checks for contact.
/// Finish or drop it before entering another phase span.
#[must_use]
pub struct FlightTrace<'a, P: Probe> {
    probe: &'a mut P,
    _span: log::tracing::span::EnteredSpan,
    launch_time_s: Real,
    sample_index: u32,
    apex_recorded: bool,
}

impl<'a, P: Probe> FlightTrace<'a, P> {
    pub fn begin(probe: &'a mut P, launch: FlightPoint) -> Self {
        if P::ENABLED {
            probe.begin_phase(Phase::Launch);
            record_point(probe, "launch", launch);
            probe.begin_phase(Phase::Flight);
            record_tunables(probe);
        }
        Self {
            probe,
            _span: log::flight().entered(),
            launch_time_s: launch.time_s,
            sample_index: 1,
            apex_recorded: false,
        }
    }

    /// Record an accepted segment, already shortened by the caller if contact occurs.
    pub fn record_step(&mut self, from: FlightPoint, to: FlightPoint) {
        if P::ENABLED {
            if !self.apex_recorded
                && let Some(apex) = from.apex_between(to)
            {
                self.record_samples(from, to, apex.time_s);
                record_point(self.probe, "apex", apex);
                self.apex_recorded = true;
            }
            self.record_samples(from, to, to.time_s);
        }
    }

    fn record_samples(&mut self, from: FlightPoint, to: FlightPoint, through_s: Real) {
        loop {
            let time_s = self.launch_time_s + Real::from(self.sample_index) * t::SAMPLE_INTERVAL_S;
            if time_s > through_s {
                break;
            }
            let fraction = (time_s - from.time_s) / (to.time_s - from.time_s);
            record_point(self.probe, "sample", from.interpolate(to, time_s, fraction));
            self.sample_index += 1;
        }
    }

    pub fn land(self, landing: FlightPoint) {
        if P::ENABLED {
            record_point(self.probe, "landing", landing);
        }
    }
}

fn record_point(probe: &mut impl Probe, event: &str, point: FlightPoint) {
    probe.value(
        &format!("{event}_time_s"),
        point.time_s,
        "s",
        Source::Computed,
    );
    for (quantity, vector, unit) in [
        ("position", point.position, "m"),
        ("velocity", point.velocity, "m/s"),
        ("spin", point.spin, "rad/s"),
    ] {
        for (axis, value) in [("x", vector.x), ("y", vector.y), ("z", vector.z)] {
            probe.value(
                &format!("{event}_{quantity}_{axis}"),
                value,
                unit,
                Source::Computed,
            );
        }
    }
}

fn record_tunables(probe: &mut impl Probe) {
    for (quantity, name, value, unit) in [
        ("drag", "CD_BASE", t::CD_BASE, "1"),
        ("drag", "CD_SPIN", t::CD_SPIN, "1"),
        ("drag", "CD_SPIN2", t::CD_SPIN2, "1"),
        ("drag", "CD_SPIN_PEAK", t::CD_SPIN_PEAK, "1"),
        ("drag", "CD_CRISIS_RE", t::CD_CRISIS_RE, "1"),
        ("drag", "CD_CRISIS_WIDTH", t::CD_CRISIS_WIDTH, "1"),
        ("drag", "CD_CRISIS_RISE", t::CD_CRISIS_RISE, "1"),
        ("lift", "CL_SCALE", t::CL_SCALE, "1"),
        ("lift", "CL_EXPONENT", t::CL_EXPONENT, "1"),
        ("gravity", "GRAVITY", t::GRAVITY, "m/s²"),
        (
            "canopy_drag",
            "CANOPY_DRAG_PER_M",
            t::CANOPY_DRAG_PER_M,
            "1/m",
        ),
        ("spin_decay", "SPIN_DECAY_LAMBDA", t::SPIN_DECAY_LAMBDA, "1"),
        (
            "canopy_spin_decay",
            "CANOPY_SPIN_LOSS_PER_S",
            t::CANOPY_SPIN_LOSS_PER_S,
            "1/s",
        ),
        (
            "sample_interval",
            "SAMPLE_INTERVAL_S",
            t::SAMPLE_INTERVAL_S,
            "s",
        ),
    ] {
        probe.value(quantity, value, unit, Source::Tunable { name: name.into() });
    }
}
