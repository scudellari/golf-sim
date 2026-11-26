//! Swept sphere contact against finite solid obstacles.

use golf_sim_base::{Real, Vec3};

use crate::{
    plane::sweep_cylinder,
    values::{ObstacleHit, ObstacleKind},
};

/// Sweeps a ball against a finite vertical cylinder, including its rounded, sphere-expanded edges.
pub fn cylinder(
    from: Vec3,
    to: Vec3,
    base: Vec3,
    height_m: Real,
    radius_m: Real,
    ball_radius_m: Real,
    kind: ObstacleKind,
) -> Option<ObstacleHit> {
    sweep_cylinder(from, to, base, height_m, radius_m, ball_radius_m, kind)
}
