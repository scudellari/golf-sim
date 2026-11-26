//! Advances a launch through flight, contact and rest on one stroke clock.

use golf_sim_base::{
    Real, Surface, Vec3, fm, log,
    num::{lerpf, smoothstep},
    trace::{Phase as TracePhase, Probe, Source},
};

use crate::{
    air::{Atmosphere, FlightAir},
    bounce, cup, flight,
    roll::{self, Motion},
    samplers::{AirSampler, GroundSample, GroundSampler, ZoneCrossing},
    tunables as t,
    values::{
        BallSpec, Cup, EndReason, EventKind, Launch, ObstacleHit, ObstacleKind, ShotEvent,
        ShotResult, Touchdown, Zone, xz,
    },
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// Round clock at the launch or release.
    pub time_s: Real,
    /// Record playback samples and event stamps. Events are always kept.
    pub playback: bool,
    /// Treat the launch as a relief drop, which must begin above the ground.
    pub release: bool,
    /// Multiplier for the integration steps. Use 1.0 for the standard solver.
    pub step_scale: Real,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            time_s: 0.0,
            playback: true,
            release: false,
            step_scale: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Phase {
    Flight,
    Ground(roll::Air),
    CupDrop { started: Real, from: Vec3, to: Vec3 },
    Done,
}

struct Run<'a> {
    result: ShotResult,
    ball: &'a BallSpec,
    conditions: &'a dyn AirSampler,
    ground: &'a dyn GroundSampler,
    current_ground: GroundSample,
    atmosphere: Atmosphere,
    planar: bool,
    uniform_flat: bool,
    maximum_grade: Real,
    max_vertical_radius: Real,
    options: Options,
    cup: Option<Cup>,
    cup_entered: bool,
    crossing_to_stick: bool,
    zone: Zone,
    zone_from: Vec3,
    zone_time: Real,
    crossings: Vec<ZoneCrossing>,
    surface: Surface,
    time: Real,
    phase: Phase,
    position: Vec3,
    velocity: Vec3,
    spin: Vec3,
    motion: Motion,
    next_sample: Real,
    next_trace_sample: Option<Real>,
    apex_done: bool,
    in_canopy: bool,
}

/// Integrate an already resolved launch until the ball comes to rest or play ends.
pub fn run<P: Probe>(
    launch: &Launch,
    ball: &BallSpec,
    conditions: &dyn AirSampler,
    ground: &dyn GroundSampler,
    cup: Option<Cup>,
    options: Options,
    probe: &mut P,
) -> ShotResult {
    let _span = log::tracing::debug_span!(log::STROKE, time_s = options.time_s).entered();
    let mut result = ShotResult {
        launch: Some(*launch),
        ..ShotResult::default()
    };
    if !launch.position.is_finite() {
        result.end_reason = if options.release {
            EndReason::InvalidRelease
        } else {
            EndReason::Timeout
        };
        return result;
    }
    if !options.step_scale.is_finite()
        || options.step_scale <= 0.0
        || !launch.velocity.is_finite()
        || !launch.spin.is_finite()
    {
        result.end_reason = EndReason::InvalidInput;
        result.rest_position = launch.position;
        result.rest_surface = ground
            .sample_ground(launch.position.x, launch.position.z)
            .surface;
        return result;
    }

    let current_ground = ground.sample_ground(launch.position.x, launch.position.z);
    let maximum_grade = ground.maximum_grade();
    let max_vertical_radius = if maximum_grade.is_finite() && maximum_grade >= 0.0 {
        ball.radius_m * fm::sqrt(1.0 + maximum_grade * maximum_grade)
    } else {
        Real::INFINITY
    };
    let mut stroke = Run {
        result,
        ball,
        conditions,
        ground,
        current_ground,
        atmosphere: Atmosphere::default(),
        planar: ground.is_planar(),
        uniform_flat: ground.is_uniform_flat(),
        maximum_grade,
        max_vertical_radius,
        options,
        cup: cup.filter(|c| c.position.is_finite()),
        cup_entered: false,
        crossing_to_stick: false,
        zone: if ground.has_zones() {
            ground.zone_at(launch.position)
        } else {
            Zone::InPlay
        },
        zone_from: launch.position,
        zone_time: 0.0,
        crossings: Vec::new(),
        surface: current_ground.surface,
        time: 0.0,
        phase: Phase::Flight,
        position: launch.position,
        velocity: launch.velocity,
        spin: launch.spin,
        motion: Motion::Skid,
        next_sample: t::SAMPLE_INTERVAL_S,
        next_trace_sample: None,
        apex_done: false,
        in_canopy: false,
    };
    stroke.leave_lie(probe);
    let mut stationary_steps = 0;
    while !matches!(stroke.phase, Phase::Done) {
        let before = stroke.time;
        let remaining = t::MAX_SHOT_TIME_S - before;
        if remaining <= 1e-9 {
            stroke.finish(EndReason::Timeout);
            break;
        }
        match stroke.phase {
            Phase::Flight => {
                stroke.flight_step((t::FLIGHT_DT * options.step_scale).min(remaining), probe)
            }
            Phase::Ground(air) => {
                let dt = if matches!(stroke.surface, Surface::Green | Surface::Fringe) {
                    t::PUTTING_GROUND_DT
                } else {
                    t::GROUND_DT
                };
                stroke.ground_step(air, (dt * options.step_scale).min(remaining), probe);
            }
            Phase::CupDrop { started, from, to } => {
                stroke.cup_drop_step(
                    started,
                    from,
                    to,
                    (t::FLIGHT_DT * options.step_scale).min(remaining),
                    probe,
                );
            }
            Phase::Done => unreachable!(),
        }
        stationary_steps = if stroke.time == before {
            stationary_steps + 1
        } else {
            0
        };
        if stationary_steps >= t::MAX_SIMULTANEOUS_CONTACTS && !matches!(stroke.phase, Phase::Done)
        {
            stroke.finish(EndReason::Timeout);
        }
    }
    stroke.result
}

impl Run<'_> {
    fn leave_lie<P: Probe>(&mut self, probe: &mut P) {
        let ground = self.current_ground;
        let clearance = clearance(self.position.y, ground, self.ball.radius_m);
        if self.options.release && clearance <= t::CONTACT_SLOP_M {
            self.finish(EndReason::InvalidRelease);
            return;
        }
        self.emit(if self.options.release {
            EventKind::Release
        } else {
            EventKind::Impact
        });
        if P::ENABLED {
            probe.begin_phase(TracePhase::Launch);
            trace_point(probe, self.time, self.position, self.velocity, self.spin);
        }
        let normal_speed = self.velocity.dot(ground.normal);
        if clearance <= t::CONTACT_SLOP_M && normal_speed < -t::CONTACT_SLOP_M {
            self.touch_down(probe);
        } else if !self.options.release
            && clearance <= t::SUPPORT_BLEND_HEIGHT_M
            && normal_speed >= -t::CONTACT_SLOP_M
        {
            self.velocity = leaving_the_lie(self.velocity, ground, clearance, normal_speed);
            if self.velocity.dot(ground.normal) <= t::CONTACT_SLOP_M {
                self.position.y = supported_height(ground, self.ball.radius_m);
                self.start_ground();
            }
        }
    }

    fn flight_step<P: Probe>(&mut self, dt: Real, probe: &mut P) {
        let _span = log::flight().entered();
        let from = self.position;
        let before = self.velocity;
        let canopy = self.ground.canopy_density(from);
        let Some(air) = self.air_at(from.y - self.current_ground.height_m, canopy) else {
            self.finish(EndReason::InvalidInput);
            return;
        };
        if (canopy > 0.0) != self.in_canopy {
            self.in_canopy = canopy > 0.0;
            if self.in_canopy {
                self.emit(EventKind::Canopy);
            }
        }
        if P::ENABLED && self.next_trace_sample.is_none() {
            probe.begin_phase(TracePhase::Flight);
            probe.value("air_density", air.density_kg_m3, "kg/m³", Source::Computed);
            for (name, value, unit) in [
                ("GRAVITY", t::GRAVITY, "m/s²"),
                ("SPIN_DECAY_LAMBDA", t::SPIN_DECAY_LAMBDA, ""),
            ] {
                probe.value(name, value, unit, Source::Tunable { name: name.into() });
            }
            trace_point(probe, self.time, from, before, self.spin);
            self.next_trace_sample = Some(self.time + t::SAMPLE_INTERVAL_S);
        }
        let step = flight::step(before, self.spin, &air, self.ball, dt);
        let to = from + step.travel;
        let next_ground = self.ground_at(to);
        let ground_fraction = self.ground_fraction(to, step.travel, next_ground);
        let hit = self.first_obstacle(to, true);
        let obstacle_fraction = hit.map_or(Real::INFINITY, |hit| hit.fraction);
        let cup_fraction = self.cup.map_or(Real::INFINITY, |cup| {
            let centre_height = cup.position.y
                + if self.cup_entered {
                    -self.ball.radius_m
                } else {
                    self.ball.radius_m
                };
            cup::crosses_rim_height(from, to, cup.position, centre_height)
        });
        let fraction = 1.0_f64
            .min(ground_fraction)
            .min(obstacle_fraction)
            .min(cup_fraction);
        let elapsed = dt * fraction;
        self.position = if fraction < 1.0 {
            from + step.travel * fraction
        } else {
            to
        };
        self.velocity = if fraction < 1.0 {
            before.lerp(step.velocity, fraction)
        } else {
            step.velocity
        };
        self.spin = flight::decay_spin(self.spin, before, self.velocity, &air, self.ball, elapsed);
        self.time += elapsed;
        self.check_apex(from, before, step.travel * fraction, elapsed, probe);
        self.track_zones();
        self.current_ground = if fraction < 1.0 {
            self.ground_at(self.position)
        } else {
            next_ground
        };
        if P::ENABLED {
            let contact = fraction == ground_fraction
                || fraction == obstacle_fraction
                || fraction == cup_fraction;
            let due = self
                .next_trace_sample
                .is_some_and(|time| self.time + 1e-9 >= time);
            if due || contact {
                trace_point(probe, self.time, self.position, self.velocity, self.spin);
            }
            if due {
                self.next_trace_sample = self
                    .next_trace_sample
                    .map(|time| time + t::SAMPLE_INTERVAL_S);
            }
        }
        drop(_span);
        if let Some(hit) = hit.filter(|hit| hit.fraction == fraction) {
            self.position += hit.normal * t::CONTACT_SLOP_M;
            self.obstacle_response(hit);
            if hit.kind == ObstacleKind::CupRim {
                self.emit(EventKind::LipOut);
                self.trace_cup("airborne_lip_out_s", probe);
                self.next_trace_sample = None;
            } else {
                self.emit(EventKind::Obstacle);
                if hit.kind == ObstacleKind::Flagstick {
                    self.trace_cup("flagstick_contact_s", probe);
                    self.next_trace_sample = None;
                }
            }
        } else if ground_fraction == fraction {
            self.touch_down(probe);
        } else if cup_fraction == fraction {
            if self.cup_entered {
                self.hole_out(probe);
            } else {
                self.cup_entered = true;
                self.emit(EventKind::CupEntry);
                self.trace_cup("airborne_entry_s", probe);
                self.next_trace_sample = None;
            }
        } else {
            self.sample_if_due();
        }
    }

    fn ground_at(&self, at: Vec3) -> GroundSample {
        if self.uniform_flat {
            self.current_ground
        } else {
            self.ground.sample_ground(at.x, at.z)
        }
    }

    fn ground_fraction(&self, to: Vec3, travel: Vec3, next_ground: GroundSample) -> Real {
        let radius = self.ball.radius_m;
        let from = self.position;
        if self.planar {
            let end = clearance(to.y, next_ground, radius);
            if end > 0.0 {
                return Real::INFINITY;
            }
            let start = clearance(from.y, self.current_ground, radius);
            let fraction = if start > 0.0 {
                start / (start - end)
            } else {
                0.0
            };
            return if self.over_open_cup(from + travel * fraction) {
                Real::INFINITY
            } else {
                fraction
            };
        }
        let length = xz(travel).length();
        // Every interior point lies at most half a segment from its nearer endpoint.
        let ceiling = self.current_ground.height_m.max(next_ground.height_m)
            + 0.5 * length * self.maximum_grade;
        if self.max_vertical_radius.is_finite()
            && from.y.min(to.y) > ceiling + self.max_vertical_radius
        {
            return Real::INFINITY;
        }
        let splits = (fm::ceil(length / t::TERRAIN_SWEEP_M) as u32).max(1);
        let mut lo = 0.0;
        for index in 1..=splits {
            let hi = Real::from(index) / Real::from(splits);
            let point = from + travel * hi;
            let ground = if index == splits {
                next_ground
            } else {
                self.ground_at(point)
            };
            if clearance(point.y, ground, radius) <= 0.0 {
                let mut left = lo;
                let mut right = hi;
                for _ in 0..t::TERRAIN_REFINE_STEPS {
                    let mid = (left + right) * 0.5;
                    let point = from + travel * mid;
                    if clearance(point.y, self.ground_at(point), radius) > 0.0 {
                        left = mid;
                    } else {
                        right = mid;
                    }
                }
                if !self.over_open_cup(from + travel * right) {
                    return right;
                }
            }
            lo = hi;
        }
        Real::INFINITY
    }

    fn over_open_cup(&self, at: Vec3) -> bool {
        self.cup.is_some_and(|cup| {
            (self.cup_entered
                || (at.y - cup.position.y - self.ball.radius_m).abs() <= self.ball.radius_m)
                && cup::within(at, cup.position, t::CUP_RADIUS_M)
        })
    }

    fn air_at(&mut self, height: Real, canopy: Real) -> Option<FlightAir> {
        let sample = self
            .conditions
            .sample_air(self.options.time_s + self.time, self.position);
        self.atmosphere.sample(&sample, height, canopy)
    }

    fn check_apex<P: Probe>(
        &mut self,
        from: Vec3,
        before: Vec3,
        travel: Vec3,
        dt: Real,
        probe: &mut P,
    ) {
        if self.apex_done
            || !self.result.touchdowns.is_empty()
            || before.y <= 0.0
            || self.velocity.y > 0.0
        {
            return;
        }
        self.apex_done = true;
        let fraction = before.y / (before.y - self.velocity.y);
        let mut point = self.position - travel * (1.0 - fraction);
        point.y = from.y + before.y * fraction * dt * 0.5;
        let time = self.time - dt * (1.0 - fraction);
        // Apex is backdated within the step and has no playback stamp.
        self.insert_event(
            EventKind::Apex,
            time,
            point,
            before.lerp(self.velocity, fraction),
        );
        if P::ENABLED {
            probe.value("apex_time_s", time, "s", Source::Computed);
            probe.value("apex_height_m", point.y, "m", Source::Computed);
        }
    }

    fn touch_down(&mut self, probe: &mut impl Probe) {
        let ground = self.current_ground;
        self.position.y = supported_height(ground, self.ball.radius_m);
        self.next_trace_sample = None;
        let first = self.result.touchdowns.is_empty();
        self.cup_entered = false;
        self.result.touchdowns.push(Touchdown {
            time_s: self.time,
            position: self.position,
            contact_point: self.position - ground.normal * self.ball.radius_m,
            velocity: self.velocity,
            depth_m: bounce::pitch_depth(-self.velocity.dot(ground.normal), &ground),
            surface: ground.surface,
        });
        self.set_surface(ground.surface);
        if first {
            self.emit(EventKind::Landing);
        }
        if ground.surface == Surface::Water {
            self.finish(EndReason::Water);
            return;
        }
        let outcome = bounce::impact(self.velocity, self.spin, &ground, self.ball, probe);
        self.velocity = outcome.velocity;
        self.spin = outcome.spin;
        if hop_height(self.velocity.dot(ground.normal), ground.normal) > t::MIN_HOP_HEIGHT_M {
            self.emit(EventKind::Bounce);
            self.position += ground.normal * t::CONTACT_SLOP_M;
        } else {
            self.start_ground();
        }
    }

    fn start_ground(&mut self) {
        let Some(air) = self.air_at(0.0, 0.0) else {
            self.finish(EndReason::InvalidInput);
            return;
        };
        self.phase = Phase::Ground(roll::Air {
            density: air.density_kg_m3,
            viscosity: air.viscosity_pa_s,
            half_area_per_mass: 0.5 * self.ball.area() / self.ball.mass_kg,
        });
        self.velocity = self.velocity.slide(self.current_ground.normal);
        self.motion = Motion::Skid;
        self.emit(EventKind::RollStart);
    }

    fn ground_step<P: Probe>(&mut self, air: roll::Air, dt: Real, probe: &mut P) {
        let ground = self.current_ground;
        let mut step = self.roll_state();
        roll::step(
            &mut step,
            &ground,
            self.ball,
            &air,
            dt,
            &mut golf_sim_base::trace::NoProbe,
        );
        let travel = step.velocity * dt;
        let mut to = self.position + travel;
        let next = self.ground_at(to);
        to.y = supported_height(next, self.ball.radius_m);
        let extra_height = if self.planar {
            0.0
        } else {
            let tangent_rise =
                -(ground.normal.x * travel.x + ground.normal.z * travel.z) / ground.normal.y;
            to.y - supported_height(ground, self.ball.radius_m) - tangent_rise
        };
        if -extra_height > t::CONTACT_SLOP_M * 0.1
            && step.velocity.dot(next.normal) > t::GRAVITY * next.normal.y * dt
        {
            self.phase = Phase::Flight;
            self.position += ground.normal * t::CONTACT_SLOP_M;
            self.flight_step(dt, probe);
            return;
        }
        let hit = self.first_obstacle(to, false);
        let obstacle_fraction = hit.map_or(Real::INFINITY, |hit| hit.fraction);
        let rim_fraction = self
            .cup
            .filter(|cup| {
                !self.cup_entered
                    && (self.position.y - cup.position.y - self.ball.radius_m).abs()
                        <= self.ball.radius_m
            })
            .map_or(Real::INFINITY, |cup| {
                cup::rim_crossing(self.position, to, cup.position)
            });
        let fraction = obstacle_fraction.min(rim_fraction);
        if fraction <= 1.0 {
            step = self.roll_state();
            roll::step(&mut step, &ground, self.ball, &air, dt * fraction, probe);
            self.time += dt * fraction;
            self.position = Vec3::new(
                self.position.x + travel.x * fraction,
                lerpf(self.position.y, to.y, fraction),
                self.position.z + travel.z * fraction,
            );
            self.apply_roll(step);
            self.track_zones();
            self.current_ground = self.ground_at(self.position);
            if let Some(hit) = hit.filter(|hit| hit.fraction <= rim_fraction) {
                self.ground_obstacle(hit, probe);
            } else {
                self.putt_entry(probe);
            }
            return;
        }
        if extra_height > 0.0
            && 2.0 * t::GRAVITY * extra_height
                > step.velocity.length_squared() * rolling_inertia(step.motion, self.ball)
        {
            self.finish(EndReason::Rest);
            return;
        }
        if P::ENABLED {
            // The speculative step may be cut short. Trace only the accepted update.
            let mut accepted = self.roll_state();
            roll::step(&mut accepted, &ground, self.ball, &air, dt, probe);
        }
        let was_motion = self.motion;
        self.time += dt;
        self.position = to;
        self.apply_roll(step);
        if !self.planar {
            self.follow_terrain(next, extra_height);
        }
        self.current_ground = next;
        self.track_zones();
        self.set_surface(next.surface);
        if next.surface == Surface::Water {
            self.finish(EndReason::Water);
            return;
        }
        if self.cup_entered
            && self
                .cup
                .is_some_and(|cup| !cup::within(self.position, cup.position, t::CUP_RADIUS_M))
        {
            self.crossing_to_stick = false;
            self.cup_entered = false;
        }
        if was_motion == Motion::Skid && self.motion == Motion::Roll {
            self.emit(EventKind::PureRoll);
        }
        if self.motion == Motion::Rest {
            self.finish(EndReason::Rest);
        } else {
            self.sample_if_due();
        }
    }

    fn ground_obstacle(&mut self, hit: ObstacleHit, probe: &mut impl Probe) {
        // Test the rebound against the surface at the hit, which can differ from the step's start.
        let normal = self.current_ground.normal;
        self.position += hit.normal * t::CONTACT_SLOP_M;
        self.obstacle_response(hit);
        self.emit(EventKind::Obstacle);
        self.motion = Motion::Skid;
        if hit.kind == ObstacleKind::Flagstick {
            self.trace_cup("flagstick_contact_s", probe);
            if self.crossing_to_stick {
                self.crossing_to_stick = false;
                if self.velocity.length() <= t::CUP_CAPTURE_SPEED {
                    self.start_cup_drop(probe);
                }
            }
        }
        if matches!(self.phase, Phase::Ground(_))
            && hop_height(self.velocity.dot(normal), normal) > t::MIN_HOP_HEIGHT_M
        {
            self.phase = Phase::Flight;
            self.position += normal * t::CONTACT_SLOP_M;
        }
    }

    fn roll_state(&self) -> roll::State {
        roll::State {
            velocity: self.velocity,
            spin: self.spin,
            motion: self.motion,
        }
    }

    fn apply_roll(&mut self, state: roll::State) {
        self.velocity = state.velocity;
        self.spin = state.spin;
        self.motion = state.motion;
    }

    fn follow_terrain(&mut self, ground: GroundSample, extra_height: Real) {
        let speed_squared = self.velocity.length_squared();
        if speed_squared <= 0.0 {
            return;
        }
        let corrected = (speed_squared
            - 2.0 * t::GRAVITY * extra_height / rolling_inertia(self.motion, self.ball))
        .max(0.0);
        self.velocity = self.velocity.slide(ground.normal).normalized() * fm::sqrt(corrected);
        if self.motion == Motion::Roll {
            self.spin = ground.normal.cross(self.velocity) / self.ball.radius_m;
        }
    }

    fn putt_entry(&mut self, probe: &mut impl Probe) {
        let cup = self.cup.expect("rim crossing requires a cup");
        self.cup_entered = true;
        self.emit(EventKind::CupEntry);
        match cup::putt_entry(self.position, self.velocity, cup, self.ball.radius_m, probe) {
            cup::PuttEntry::Capture => self.start_cup_drop(probe),
            cup::PuttEntry::LipOut { velocity } => {
                self.velocity = velocity;
                self.motion = Motion::Skid;
                self.emit(EventKind::LipOut);
            }
            cup::PuttEntry::CrossingToFlagstick => self.crossing_to_stick = true,
        }
    }

    fn start_cup_drop(&mut self, probe: &mut impl Probe) {
        let cup = self.cup.expect("capture requires a cup");
        let mut to = cup.position;
        to.y -= self.ball.radius_m;
        if cup.flagstick_in {
            let side = xz(self.position - cup.position).normalized()
                * ((t::CUP_RADIUS_M + t::FLAGSTICK_RADIUS_M) * 0.5);
            to.x += side.x;
            to.z += side.y;
        }
        self.trace_cup("drop_started_s", probe);
        self.phase = Phase::CupDrop {
            started: self.time,
            from: self.position,
            to,
        };
    }

    fn cup_drop_step(
        &mut self,
        started: Real,
        from: Vec3,
        to: Vec3,
        dt: Real,
        probe: &mut impl Probe,
    ) {
        let _span = log::cup().entered();
        // The capture curve already accounts for the rim, so only solids can interrupt the drop.
        let elapsed = self.time - started;
        let duration = fm::sqrt(2.0 * (from.y - to.y).max(0.0) / t::GRAVITY);
        let next = (elapsed + dt).min(duration);
        let fraction = if duration > 0.0 { next / duration } else { 1.0 };
        let mut point = from.lerp(to, fraction);
        point.y = from.y - 0.5 * t::GRAVITY * next * next;
        self.velocity = (point - self.position) / (next - elapsed).max(1e-9);
        if let Some(hit) = self.first_obstacle(point, false) {
            self.position = self.position.lerp(point, hit.fraction);
            self.position.y = hit.point.y;
            self.position += hit.normal * t::CONTACT_SLOP_M;
            self.time += (next - elapsed) * hit.fraction;
            self.obstacle_response(hit);
            self.emit(EventKind::Obstacle);
            self.phase = Phase::Flight;
            self.current_ground = self.ground_at(self.position);
            return;
        }
        self.position = point;
        self.time = started + next;
        if next == duration {
            self.hole_out(probe);
        } else {
            self.sample_if_due();
        }
    }

    fn hole_out(&mut self, probe: &mut impl Probe) {
        self.trace_cup("holed_at_s", probe);
        self.emit(EventKind::Holed);
        self.finish(EndReason::Holed);
    }

    fn trace_cup<P: Probe>(&self, name: &str, probe: &mut P) {
        if P::ENABLED {
            probe.begin_phase(TracePhase::Cup);
            probe.value(name, self.time, "s", Source::Computed);
        }
    }

    fn first_obstacle(&self, to: Vec3, flying: bool) -> Option<ObstacleHit> {
        let mut hit = self
            .ground
            .obstacle_hit(self.position, to, self.ball.radius_m);
        if let Some(cup) = self.cup {
            // Ties go to the course obstacle, the stick, the rim and then the ground.
            if let Some(stick) = cup::flagstick_hit(self.position, to, cup, self.ball.radius_m)
                && hit.is_none_or(|old| stick.fraction < old.fraction)
            {
                hit = Some(stick);
            }
            if flying
                && let Some(rim) = cup::rim_hit(self.position, to, cup.position, self.ball.radius_m)
                && hit.is_none_or(|old| rim.fraction < old.fraction)
            {
                hit = Some(rim);
            }
        }
        hit
    }

    fn obstacle_response(&mut self, hit: ObstacleHit) {
        let props = t::obstacle(hit.kind);
        let normal = hit.normal;
        let incoming = self.velocity.dot(normal);
        if incoming >= 0.0 {
            return;
        }
        let radius = self.ball.radius_m;
        let inertia = self.ball.inertia_factor;
        let arm = -normal * radius;
        let slip = (self.velocity + self.spin.cross(arm)).slide(normal);
        let impulse = -(1.0 + props.cor) * incoming;
        let friction = (-slip * (inertia / (1.0 + inertia))).limit_length(props.friction * impulse);
        self.velocity += normal * impulse + friction;
        self.spin += arm.cross(friction) / (inertia * radius * radius);
    }

    fn track_zones(&mut self) {
        if !self.ground.has_zones() {
            return;
        }
        self.ground.crossings(
            self.zone_from,
            self.position,
            self.zone,
            &mut self.crossings,
        );
        for index in 0..self.crossings.len() {
            let crossing = self.crossings[index];
            self.zone = crossing.zone;
            let time = lerpf(self.zone_time, self.time, crossing.fraction);
            let point = self.zone_from.lerp(self.position, crossing.fraction);
            self.insert_event(EventKind::ZoneEntered, time, point, self.velocity);
            self.record_at(time, point);
        }
        self.zone_from = self.position;
        self.zone_time = self.time;
    }

    fn set_surface(&mut self, surface: Surface) {
        if surface != self.surface {
            self.surface = surface;
            self.emit(EventKind::SurfaceEntered);
        }
    }

    fn emit(&mut self, kind: EventKind) {
        self.insert_event(kind, self.time, self.position, self.velocity);
        self.record_at(self.time, self.position);
    }

    fn insert_event(&mut self, kind: EventKind, time: Real, position: Vec3, velocity: Vec3) {
        let index = self
            .result
            .events
            .partition_point(|event| event.time_s <= time);
        self.result.events.insert(
            index,
            ShotEvent {
                kind,
                time_s: time,
                position,
                velocity,
                spin: self.spin,
                surface: self.surface,
                zone: self.zone,
            },
        );
    }

    fn record_at(&mut self, time: Real, position: Vec3) {
        if !self.options.playback {
            return;
        }
        let index = self.result.times.partition_point(|&stamp| stamp < time);
        if self.result.times.get(index) == Some(&time) {
            self.result.positions[index] = position;
        } else {
            self.result.times.insert(index, time);
            self.result.positions.insert(index, position);
        }
    }

    fn sample_if_due(&mut self) {
        if self.options.playback && self.time + 1e-9 >= self.next_sample {
            self.record_at(self.time, self.position);
            self.next_sample += t::SAMPLE_INTERVAL_S;
        }
    }

    fn finish(&mut self, reason: EndReason) {
        self.phase = Phase::Done;
        self.result.end_reason = reason;
        self.result.rest_position = self.position;
        let ground = self.ground_at(self.position);
        self.result.rest_surface = ground.surface;
        if reason == EndReason::Timeout {
            self.result.rest_position.y = supported_height(ground, self.ball.radius_m);
        }
        if reason == EndReason::Rest {
            self.velocity = Vec3::ZERO;
            self.spin = Vec3::ZERO;
            self.emit(EventKind::Rest);
        } else {
            self.record_at(self.time, self.position);
        }
    }
}

fn clearance(y: Real, ground: GroundSample, radius: Real) -> Real {
    (y - ground.height_m) * ground.normal.y - radius
}

fn supported_height(ground: GroundSample, radius: Real) -> Real {
    ground.height_m + radius / ground.normal.y
}

fn leaving_the_lie(
    velocity: Vec3,
    ground: GroundSample,
    clearance: Real,
    normal_speed: Real,
) -> Vec3 {
    let supported = 1.0
        - smoothstep(
            t::SUPPORTED_LIE_HEIGHT_M,
            t::SUPPORT_BLEND_HEIGHT_M,
            clearance,
        );
    let settle_speed = fm::sqrt(2.0 * t::GRAVITY * ground.normal.y * t::MIN_HOP_HEIGHT_M);
    let absorbed = normal_speed.min(settle_speed)
        * supported
        * (1.0 - smoothstep(settle_speed, 3.0 * settle_speed, normal_speed));
    velocity - ground.normal * absorbed
}

fn hop_height(normal_speed: Real, normal: Vec3) -> Real {
    let upward = normal_speed.max(0.0);
    upward * upward / (2.0 * t::GRAVITY * normal.y)
}

fn rolling_inertia(motion: Motion, ball: &BallSpec) -> Real {
    if motion == Motion::Roll {
        1.0 + ball.inertia_factor
    } else {
        1.0
    }
}

fn trace_point(probe: &mut impl Probe, time: Real, position: Vec3, velocity: Vec3, spin: Vec3) {
    for (name, value, unit) in [
        ("time_s", time, "s"),
        ("x", position.x, "m"),
        ("y", position.y, "m"),
        ("z", position.z, "m"),
        ("speed", velocity.length(), "m/s"),
        ("spin", spin.length(), "rad/s"),
    ] {
        probe.value(name, value, unit, Source::Computed);
    }
}

/// Horizontal carry to a level surface, without contacts or canopy. Returns NaN
/// if the ball never descends through the requested level.
pub fn carry_to_height(
    launch: &Launch,
    ball: &BallSpec,
    time_s: Real,
    conditions: &dyn AirSampler,
    ground: &dyn GroundSampler,
    height_m: Real,
) -> Real {
    let mut position = launch.position;
    let mut velocity = launch.velocity;
    let mut spin = launch.spin;
    let initial_ground = ground.sample_ground(position.x, position.z);
    let clearance = clearance(position.y, initial_ground, ball.radius_m);
    let normal_speed = velocity.dot(initial_ground.normal);
    if clearance <= t::CONTACT_SLOP_M && normal_speed < -t::CONTACT_SLOP_M {
        return Real::NAN;
    }
    if clearance <= t::SUPPORT_BLEND_HEIGHT_M && normal_speed >= -t::CONTACT_SLOP_M {
        velocity = leaving_the_lie(velocity, initial_ground, clearance, normal_speed);
        if velocity.dot(initial_ground.normal) <= t::CONTACT_SLOP_M {
            return Real::NAN;
        }
    }
    let uniform_flat = ground.is_uniform_flat();
    let level = height_m + ball.radius_m;
    let mut time = 0.0;
    let mut atmosphere = Atmosphere::default();
    while time < t::MAX_SHOT_TIME_S {
        let sample = conditions.sample_air(time_s + time, position);
        let ground = if uniform_flat {
            initial_ground
        } else {
            ground.sample_ground(position.x, position.z)
        };
        let Some(air) = atmosphere.sample(&sample, position.y - ground.height_m, 0.0) else {
            return Real::NAN;
        };
        let step = flight::step(velocity, spin, &air, ball, t::FLIGHT_DT);
        let next = position + step.travel;
        if next.y <= level {
            if position.y > level {
                let fraction = (position.y - level) / (position.y - next.y);
                return (xz(position - launch.position + step.travel * fraction)).length();
            }
            if step.velocity.y <= 0.0 {
                return Real::NAN;
            }
        }
        spin = flight::decay_spin(spin, velocity, step.velocity, &air, ball, t::FLIGHT_DT);
        position = next;
        velocity = step.velocity;
        time += t::FLIGHT_DT;
    }
    Real::NAN
}
