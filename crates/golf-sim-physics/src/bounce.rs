//! Ground impact on the effective plane of the crater.

use golf_sim_base::{
    Real, Surface, Vec3, fm, log,
    num::{clampf, deg_to_rad, lerpf, rad_to_deg, smoothstep},
    trace::{Phase, Probe, Source},
};

use crate::{
    samplers::GroundSample,
    tunables as t,
    values::{BallSpec, units},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Outcome {
    pub velocity: Vec3,
    pub spin: Vec3,
}

fn crater_load(normal_speed: Real) -> Real {
    smoothstep(t::CRATER_ONSET_MPS, t::CRATER_FULL_LOAD_MPS, normal_speed)
}

fn crater_softness(ground: &GroundSample) -> Real {
    lerpf(
        t::BOUNCE_CRATER_SOFT_SCALE,
        t::BOUNCE_CRATER_FIRM_SCALE,
        clampf(ground.firmness, 0.0, 1.0),
    )
}

/// Crater tilt for an incidence angle measured from the inward ground normal.
pub fn crater_angle(
    speed: Real,
    normal_speed: Real,
    incidence_rad: Real,
    ground: &GroundSample,
) -> Real {
    let props = t::surface(ground.surface);
    let base = deg_to_rad(t::BOUNCE_CRATER_DEG)
        * (speed / t::BOUNCE_CRATER_SPEED)
        * (rad_to_deg(incidence_rad) / t::BOUNCE_CRATER_INCIDENCE_DEG);
    let softness = crater_softness(ground);
    let (scale, cap) = if ground.surface == Surface::Green {
        (
            t::BOUNCE_GREEN_CRATER_SCALE,
            t::BOUNCE_GREEN_MAX_DEG * lerpf(1.0, softness, t::BOUNCE_GREEN_CAP_SOFTNESS),
        )
    } else {
        (t::BOUNCE_CRATER_SCALE, t::BOUNCE_CRATER_MAX_DEG)
    };
    (base * softness * props.crater * scale).min(deg_to_rad(cap)) * crater_load(normal_speed)
}

pub fn pitch_depth(normal_speed: Real, ground: &GroundSample) -> Real {
    let wet = (1.0 + t::PITCH_MOISTURE_GAIN * clampf(ground.moisture, 0.0, 1.0))
        / (1.0 + t::PITCH_MOISTURE_GAIN * t::MOISTURE_REFERENCE);
    t::PITCH_DEPTH_PER_MPS
        * normal_speed
        * crater_load(normal_speed)
        * crater_softness(ground)
        * wet
        * t::surface(ground.surface).crater
}

pub fn restitution(normal_speed: Real, ground: &GroundSample) -> Real {
    let props = t::surface(ground.surface);
    if props.hard {
        return props.cor;
    }
    let mut cor = if normal_speed <= t::BOUNCE_COR_KNEE_SPEED {
        t::BOUNCE_COR_A
            + t::BOUNCE_COR_B * normal_speed
            + t::BOUNCE_COR_C * normal_speed * normal_speed
    } else {
        t::BOUNCE_COR_HIGH_SPEED
    };
    if ground.surface == Surface::Green {
        cor = cor.max(t::BOUNCE_GREEN_COR_FLOOR);
    }
    let firm = lerpf(
        t::BOUNCE_COR_SOFT_SCALE,
        t::BOUNCE_COR_FIRM_SCALE,
        clampf(ground.firmness, 0.0, 1.0),
    );
    let wet = 1.0 - t::BOUNCE_COR_WET_LOSS * clampf(ground.moisture, 0.0, 1.0);
    clampf(cor * firm * wet * props.cor, 0.0, 0.95)
}

/// Resolves one impact using metres per second and radians per second.
pub fn impact<P: Probe>(
    velocity: Vec3,
    spin: Vec3,
    ground: &GroundSample,
    ball: &BallSpec,
    probe: &mut P,
) -> Outcome {
    let _span = log::bounce().entered();
    let normal = ground.normal;
    let normal_speed = -velocity.dot(normal);
    let tangent = velocity + normal * normal_speed;
    let tangent_speed = tangent.length();
    let speed = velocity.length();
    let (plane, crater_tilt) = if tangent_speed > 1e-6 {
        let incidence = fm::atan2(tangent_speed, normal_speed);
        let tilt = crater_angle(speed, normal_speed, incidence, ground);
        let axis = normal.cross(tangent.normalized()).normalized();
        (normal.rotated(axis, -tilt), tilt)
    } else {
        (normal, 0.0)
    };
    let approach = -velocity.dot(plane);
    if approach <= 0.0 {
        return Outcome { velocity, spin };
    }

    let props = t::surface(ground.surface);
    let cor = restitution(approach, ground);
    let normal_impulse = ball.mass_kg * (1.0 + cor) * approach;
    let contact = -plane * ball.radius_m;
    let slip = (velocity + spin.cross(contact)).slide(plane);
    let (tangent_cor, tangent_source) = if ground.surface == Surface::Green {
        (
            t::BOUNCE_GREEN_TANGENTIAL_COR,
            "BOUNCE_GREEN_TANGENTIAL_COR",
        )
    } else {
        (t::BOUNCE_TANGENTIAL_COR, "BOUNCE_TANGENTIAL_COR")
    };
    let k = ball.inertia_factor;
    let to_roll = slip * (-(1.0 + tangent_cor) * ball.mass_kg * k / (1.0 + k));
    let friction_limit = props.friction * normal_impulse;
    let tangent_impulse = if to_roll.length() > friction_limit {
        to_roll.normalized() * friction_limit
    } else {
        to_roll
    };
    let impulse = plane * normal_impulse + tangent_impulse;
    let softness = (0.5 - clampf(ground.firmness, 0.0, 1.0)).max(0.0);
    let wetness = (clampf(ground.moisture, 0.0, 1.0) - t::MOISTURE_REFERENCE).max(0.0);
    let sink = props.crater
        * (softness * t::BOUNCE_SOFT_FIRMNESS_LOSS + wetness * t::BOUNCE_SOFT_MOISTURE_LOSS);
    let speed_keep = fm::sqrt(lerpf(
        1.0,
        (1.0 - props.landing_loss) * fm::exp(-sink),
        crater_load(normal_speed),
    ));
    let out = Outcome {
        velocity: (velocity + impulse / ball.mass_kg) * speed_keep,
        spin: (spin + contact.cross(impulse) / ball.moment_of_inertia()) * speed_keep,
    };

    if P::ENABLED {
        probe.begin_phase(Phase::Bounce);
        for (name, value, unit) in [
            ("incidence_speed", speed, "m/s"),
            (
                "incidence_angle",
                rad_to_deg(fm::atan2(normal_speed, tangent_speed)),
                "deg",
            ),
            ("normal_speed_at_crater", approach, "m/s"),
            ("crater_tilt", rad_to_deg(crater_tilt), "deg"),
            ("normal_restitution", cor, ""),
            ("spin_in", units::to_rpm(spin.length()), "rpm"),
            ("spin_out", units::to_rpm(out.spin.length()), "rpm"),
            ("spin_in_x", spin.x, "rad/s"),
            ("spin_in_y", spin.y, "rad/s"),
            ("spin_in_z", spin.z, "rad/s"),
            ("spin_out_x", out.spin.x, "rad/s"),
            ("spin_out_y", out.spin.y, "rad/s"),
            ("spin_out_z", out.spin.z, "rad/s"),
        ] {
            probe.value(name, value, unit, Source::Computed);
        }
        probe.value(
            "surface",
            ground.surface as u8 as Real,
            "",
            Source::Turf {
                name: ground.surface.name().into(),
            },
        );
        for (name, value) in [
            ("firmness", ground.firmness),
            ("moisture", ground.moisture),
            ("cor", props.cor),
            ("friction", props.friction),
            ("crater", props.crater),
            ("landing_loss", props.landing_loss),
        ] {
            probe.value(name, value, "", Source::Turf { name: name.into() });
        }
        let mut tunable = |field, name: &str, value, unit| {
            probe.value(field, value, unit, Source::Tunable { name: name.into() });
        };
        tunable("tangential_restitution", tangent_source, tangent_cor, "");
        if !props.hard {
            let coefficients: &[(&str, Real)] = if approach <= t::BOUNCE_COR_KNEE_SPEED {
                &[
                    ("BOUNCE_COR_A", t::BOUNCE_COR_A),
                    ("BOUNCE_COR_B", t::BOUNCE_COR_B),
                    ("BOUNCE_COR_C", t::BOUNCE_COR_C),
                ]
            } else {
                &[("BOUNCE_COR_HIGH_SPEED", t::BOUNCE_COR_HIGH_SPEED)]
            };
            for &(name, value) in coefficients.iter().chain(&[
                ("BOUNCE_COR_SOFT_SCALE", t::BOUNCE_COR_SOFT_SCALE),
                ("BOUNCE_COR_FIRM_SCALE", t::BOUNCE_COR_FIRM_SCALE),
                ("BOUNCE_COR_WET_LOSS", t::BOUNCE_COR_WET_LOSS),
            ]) {
                tunable(name, name, value, "");
            }
            tunable(
                "cor_knee_speed",
                "BOUNCE_COR_KNEE_SPEED",
                t::BOUNCE_COR_KNEE_SPEED,
                "m/s",
            );
            if ground.surface == Surface::Green {
                tunable(
                    "green_cor_floor",
                    "BOUNCE_GREEN_COR_FLOOR",
                    t::BOUNCE_GREEN_COR_FLOOR,
                    "",
                );
            }
        }
        for (name, value, unit) in [
            (
                "BOUNCE_SOFT_FIRMNESS_LOSS",
                t::BOUNCE_SOFT_FIRMNESS_LOSS,
                "",
            ),
            (
                "BOUNCE_SOFT_MOISTURE_LOSS",
                t::BOUNCE_SOFT_MOISTURE_LOSS,
                "",
            ),
            ("MOISTURE_REFERENCE", t::MOISTURE_REFERENCE, ""),
            ("CRATER_ONSET_MPS", t::CRATER_ONSET_MPS, "m/s"),
            ("CRATER_FULL_LOAD_MPS", t::CRATER_FULL_LOAD_MPS, "m/s"),
            ("BOUNCE_CRATER_DEG", t::BOUNCE_CRATER_DEG, "deg"),
            ("BOUNCE_CRATER_SPEED", t::BOUNCE_CRATER_SPEED, "m/s"),
            (
                "BOUNCE_CRATER_INCIDENCE_DEG",
                t::BOUNCE_CRATER_INCIDENCE_DEG,
                "deg",
            ),
            ("BOUNCE_CRATER_SOFT_SCALE", t::BOUNCE_CRATER_SOFT_SCALE, ""),
            ("BOUNCE_CRATER_FIRM_SCALE", t::BOUNCE_CRATER_FIRM_SCALE, ""),
        ] {
            tunable(name, name, value, unit);
        }
        let crater_tunables: &[(&str, Real, &str)] = if ground.surface == Surface::Green {
            &[
                (
                    "BOUNCE_GREEN_CRATER_SCALE",
                    t::BOUNCE_GREEN_CRATER_SCALE,
                    "",
                ),
                ("BOUNCE_GREEN_MAX_DEG", t::BOUNCE_GREEN_MAX_DEG, "deg"),
                (
                    "BOUNCE_GREEN_CAP_SOFTNESS",
                    t::BOUNCE_GREEN_CAP_SOFTNESS,
                    "",
                ),
            ]
        } else {
            &[
                ("BOUNCE_CRATER_SCALE", t::BOUNCE_CRATER_SCALE, ""),
                ("BOUNCE_CRATER_MAX_DEG", t::BOUNCE_CRATER_MAX_DEG, "deg"),
            ]
        };
        for &(name, value, unit) in crater_tunables {
            tunable(name, name, value, unit);
        }
    }
    out
}
