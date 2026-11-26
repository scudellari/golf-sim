//! Cup and flagstick contact for rolling and airborne shots.

use golf_sim_base::{
    Real, Seed, Vec3, fm, log,
    seed::SeedStream,
    trace::{NoProbe, Phase, Probe, Source},
};

use crate::{
    integrator::{self, Options},
    plane::{ConstantAir, PlaneGround},
    sweep, tunables as t,
    values::{Cup, EndReason, Launch, LaunchReadout, ObstacleHit, ObstacleKind, UP, xz},
};

pub fn within(position: Vec3, cup: Vec3, radius: Real) -> bool {
    xz(position - cup).length_squared() < radius * radius
}

/// First horizontal entry into the rim, or infinity if there is no crossing.
pub fn rim_crossing(from: Vec3, to: Vec3, cup: Vec3) -> Real {
    let offset = xz(from - cup);
    let delta = xz(to - from);
    let length_squared = delta.length_squared();
    if length_squared == 0.0 {
        return Real::INFINITY;
    }
    let radial = offset.length_squared() - t::CUP_RADIUS_M * t::CUP_RADIUS_M;
    if radial < 0.0 {
        return 0.0;
    }
    let half_b = offset.dot(delta);
    let discriminant = half_b * half_b - length_squared * radial;
    if discriminant < 0.0 {
        return Real::INFINITY;
    }
    let fraction = (-half_b - fm::sqrt(discriminant)) / length_squared;
    if (0.0..=1.0).contains(&fraction) {
        fraction
    } else {
        Real::INFINITY
    }
}

/// Downward crossing of a ball-centre height within the opening, or infinity.
pub fn crosses_rim_height(from: Vec3, to: Vec3, cup: Vec3, centre_height: Real) -> Real {
    if from.y < centre_height || to.y > centre_height || to.y >= from.y {
        return Real::INFINITY;
    }
    let fraction = (from.y - centre_height) / (from.y - to.y);
    if within(from.lerp(to, fraction), cup, t::CUP_RADIUS_M) {
        fraction
    } else {
        Real::INFINITY
    }
}

/// Sweeps a ball against the circular edge of the cup.
pub fn rim_hit(from: Vec3, to: Vec3, cup: Vec3, radius: Real) -> Option<ObstacleHit> {
    let reach = t::CUP_RADIUS_M + radius;
    if from.x.min(to.x) > cup.x + reach
        || from.x.max(to.x) < cup.x - reach
        || from.z.min(to.z) > cup.z + reach
        || from.z.max(to.z) < cup.z - reach
        || from.y.min(to.y) > cup.y + radius
        || from.y.max(to.y) < cup.y - radius
    {
        return None;
    }
    let delta = to - from;
    let length = delta.length();
    if length == 0.0 {
        return None;
    }

    let mut fraction = 0.0;
    for _ in 0..t::SOLID_SWEEP_ITERATIONS {
        let point = from.lerp(to, fraction);
        let radial = point - cup;
        let mut direction = Vec3::new(radial.x, 0.0, radial.z).normalized();
        if direction == Vec3::ZERO {
            direction = Vec3::new(1.0, 0.0, 0.0);
        }
        let offset = point - (cup + direction * t::CUP_RADIUS_M);
        let clearance = offset.length() - radius;
        if clearance <= t::CONTACT_SLOP_M * 0.1 {
            let normal = offset.normalized();
            return (delta.dot(normal) < 0.0).then_some(ObstacleHit {
                kind: ObstacleKind::CupRim,
                point,
                normal,
                fraction,
            });
        }
        fraction += clearance / length;
        if fraction > 1.0 {
            break;
        }
    }
    None
}

pub fn putt_capture_speed(lateral_offset_m: Real) -> Real {
    let offset = lateral_offset_m / t::CUP_RADIUS_M;
    t::CUP_CAPTURE_SPEED * fm::pow((1.0 - offset * offset).max(0.0), t::CUP_CAPTURE_EXPONENT)
}

pub fn putt_capture_speed_on_slope(lateral_offset_m: Real, uphill_sine: Real) -> Real {
    // Penner (2002), eq. 30. The far rim's rise reduces the required drop by
    // one ball radius times the signed uphill sine. Bound the small-slope
    // approximation at the steepest slope Penner quantified.
    let max_sine = fm::sin(t::CUP_CAPTURE_MAX_SLOPE_RAD);
    let drop_ratio = 1.0 - uphill_sine.clamp(-max_sine, max_sine);
    putt_capture_speed(lateral_offset_m) / fm::sqrt(drop_ratio)
}

pub fn captures_flagstick_rebound(position: Vec3, velocity: Vec3, cup: Cup) -> bool {
    // The outgoing line can cross a shorter chord of the opening after a glancing hit.
    let lateral = xz(position - cup.position)
        .slide(xz(velocity).normalized())
        .length();
    velocity.length() <= putt_capture_speed(lateral)
}

pub fn flagstick_hit(from: Vec3, to: Vec3, cup: Cup, ball_radius: Real) -> Option<ObstacleHit> {
    if !cup.flagstick_in {
        return None;
    }
    sweep::cylinder(
        from,
        to,
        cup.position - UP * t::CUP_DEPTH_M,
        t::FLAGSTICK_HEIGHT_M + t::CUP_DEPTH_M,
        t::FLAGSTICK_RADIUS_M,
        ball_radius,
        ObstacleKind::Flagstick,
    )
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PuttEntry {
    Capture,
    LipOut { velocity: Vec3 },
    CrossingToFlagstick,
}

/// Resolves a rolling ball at the cup entrance. A path into the stick remains live
/// so the caller can advance the ball to actual contact.
pub fn putt_entry<P: Probe>(
    position: Vec3,
    velocity: Vec3,
    cup: Cup,
    ball_radius: Real,
    probe: &mut P,
) -> PuttEntry {
    putt_entry_on_slope(position, velocity, cup, ball_radius, UP, probe)
}

pub(crate) fn putt_entry_on_slope<P: Probe>(
    position: Vec3,
    velocity: Vec3,
    cup: Cup,
    ball_radius: Real,
    normal: Vec3,
    probe: &mut P,
) -> PuttEntry {
    let _span = log::cup().entered();
    let horizontal = Vec3::new(velocity.x, 0.0, velocity.z);
    let speed = horizontal.length();
    let direction = horizontal.normalized();
    let to_centre = cup.position - position;
    let offset = Vec3::new(to_centre.x, 0.0, to_centre.z).slide(direction);
    let lateral = offset.length();
    let capture_speed = putt_capture_speed_on_slope(lateral, -normal.dot(direction));

    let outcome = if flagstick_hit(
        position,
        position + direction * (2.0 * t::CUP_RADIUS_M),
        cup,
        ball_radius,
    )
    .is_some()
    {
        PuttEntry::CrossingToFlagstick
    } else if speed <= capture_speed {
        PuttEntry::Capture
    } else {
        let turn = t::CUP_LIP_MAX_DEFLECT_RAD * (lateral / t::CUP_RADIUS_M).clamp(0.0, 1.0);
        let (sin, cos) = fm::sincos(turn);
        let velocity =
            (direction * cos - offset.normalized() * sin) * speed * t::CUP_LIP_SPEED_KEEP;
        PuttEntry::LipOut { velocity }
    };

    if P::ENABLED {
        let event = match outcome {
            PuttEntry::Capture => "capture",
            PuttEntry::LipOut { .. } => "lip_out",
            PuttEntry::CrossingToFlagstick => "crossing_to_flagstick",
        };
        probe.begin_phase(Phase::Cup);
        probe.value("rim_speed", speed, "m/s", Source::Computed);
        probe.value(event, 1.0, "", Source::Computed);
        probe.value("lateral_offset", lateral, "m", Source::Computed);
        probe.value("capture_speed", capture_speed, "m/s", Source::Computed);
        probe.value(
            "cup_radius",
            t::CUP_RADIUS_M,
            "m",
            Source::Tunable {
                name: "CUP_RADIUS_M".into(),
            },
        );
        probe.value(
            "centre_capture_speed",
            t::CUP_CAPTURE_SPEED,
            "m/s",
            Source::Tunable {
                name: "CUP_CAPTURE_SPEED".into(),
            },
        );
        probe.value(
            "capture_exponent",
            t::CUP_CAPTURE_EXPONENT,
            "",
            Source::Tunable {
                name: "CUP_CAPTURE_EXPONENT".into(),
            },
        );
        probe.value(
            "flagstick_in",
            Real::from(cup.flagstick_in),
            "",
            Source::Course {
                name: "flagstick_in".into(),
            },
        );
        if matches!(outcome, PuttEntry::LipOut { .. }) {
            probe.value(
                "lip_max_deflection",
                t::CUP_LIP_MAX_DEFLECT_RAD,
                "rad",
                Source::Tunable {
                    name: "CUP_LIP_MAX_DEFLECT_RAD".into(),
                },
            );
            probe.value(
                "lip_speed_keep",
                t::CUP_LIP_SPEED_KEEP,
                "",
                Source::Tunable {
                    name: "CUP_LIP_SPEED_KEEP".into(),
                },
            );
        }
    }
    outcome
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PuttFlagstick {
    Out,
    In,
    Stiff,
}

impl PuttFlagstick {
    pub fn trial_cell(line: &str, flagstick: &str) -> Option<(Real, Self)> {
        let offset = match line {
            "centre" => 0.0,
            // Bilder gives no offset for the off-centre putts.
            "off-centre" => t::CUP_RADIUS_M * 0.5,
            _ => return None,
        };
        let stick = match flagstick {
            "out" => Self::Out,
            "in" => Self::In,
            "in-stiff" => Self::Stiff,
            _ => return None,
        };
        Some((offset, stick))
    }
}

/// Replays a MyGolfSpy cell on a plane. Calibrate the launch against the
/// no-cup runout, then vary the entry line by 2 mm standard deviation.
pub fn replay_putts(
    ground: &PlaneGround,
    air: &ConstantAir,
    past_m: Real,
    offset_m: Real,
    flagstick: PuttFlagstick,
) -> Option<Real> {
    const START_M: Real = 2.0;
    const PUTTS: u32 = 200;
    let putt = |speed, offset, cup| {
        let launch = Launch::from_readout(
            &t::BALL,
            Vec3::new(offset, 0.0, START_M),
            Vec3::new(0.0, 0.0, -1.0),
            LaunchReadout {
                ball_speed_mps: speed,
                spin_rate_rad_s: -speed / t::BALL.radius_m,
                ..LaunchReadout::default()
            },
        );
        integrator::run_with_flagstick(
            &launch,
            &t::BALL,
            air,
            ground,
            cup,
            Options {
                playback: false,
                ..Options::default()
            },
            &mut NoProbe,
        )
    };
    let distance = START_M + past_m;
    let (mut low, mut high) = (0.0, 4.0);
    if !(0.0..=putt(high, 0.0, None).total_m()).contains(&distance) {
        return None;
    }
    for _ in 0..40 {
        let speed = (low + high) * 0.5;
        if putt(speed, 0.0, None).total_m() < distance {
            low = speed;
        } else {
            high = speed;
        }
    }
    let speed = (low + high) * 0.5;
    let cup = Cup {
        position: Vec3::ZERO,
        flagstick_in: flagstick != PuttFlagstick::Out,
    };
    let restitution = match flagstick {
        PuttFlagstick::Out | PuttFlagstick::In => t::FLAGSTICK_RESTITUTION,
        PuttFlagstick::Stiff => t::FLAGSTICK_STIFF_RESTITUTION,
    };
    let mut jitter = SeedStream::new(Seed(65));
    let holed = (0..PUTTS)
        .filter(|_| {
            let offset = offset_m + 0.002 * jitter.next_gaussian();
            putt(speed, offset, Some((cup, restitution))).end_reason == EndReason::Holed
        })
        .count();
    Some(holed as Real / Real::from(PUTTS))
}
