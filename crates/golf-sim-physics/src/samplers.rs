//! Read-only world queries used by the solver.

use golf_sim_base::{Real, Surface, Vec3};

use crate::values::{ObstacleHit, UP, Zone};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundSample {
    pub surface: Surface,
    pub height_m: Real,
    pub normal: Vec3,
    pub firmness: Real,
    pub moisture: Real,
    /// Stimpmeter reading in metres. Zero uses the surface default.
    pub green_speed_m: Real,
    /// Negative uses the surface default.
    pub grass_height_m: Real,
    /// Negative uses the lie or the surface default.
    pub grass_density: Real,
}

impl Default for GroundSample {
    fn default() -> Self {
        Self {
            surface: Surface::Fairway,
            height_m: 0.0,
            normal: UP,
            firmness: 0.5,
            moisture: 0.3,
            green_speed_m: 0.0,
            grass_height_m: -1.0,
            grass_density: -1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AirSample {
    pub temperature_c: Real,
    /// Station pressure in pascals.
    pub pressure_pa: Real,
    pub relative_humidity: Real,
    pub mean_wind_mps: Vec3,
    pub gust_mps: Vec3,
    pub wind_reference_height_m: Real,
    pub roughness_length_m: Real,
}

impl Default for AirSample {
    fn default() -> Self {
        Self {
            temperature_c: 15.0,
            pressure_pa: 101325.0,
            relative_humidity: 0.5,
            mean_wind_mps: Vec3::ZERO,
            gust_mps: Vec3::ZERO,
            wind_reference_height_m: 10.0,
            roughness_length_m: 0.03,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoneCrossing {
    pub fraction: Real,
    pub zone: Zone,
}

/// Deterministic ground queries over a fixed snapshot for one stroke.
pub trait GroundSampler {
    fn sample_ground(&self, x: Real, z: Real) -> GroundSample;

    fn obstacle_hit(&self, _from: Vec3, _to: Vec3, _radius_m: Real) -> Option<ObstacleHit> {
        None
    }

    fn canopy_density(&self, _position: Vec3) -> Real {
        0.0
    }

    fn has_zones(&self) -> bool {
        false
    }

    fn zone_at(&self, _position: Vec3) -> Zone {
        Zone::InPlay
    }

    /// Replaces `out` with the boundaries crossed in segment order, at fractions in (0, 1].
    fn crossings(&self, _from: Vec3, _to: Vec3, _from_zone: Zone, out: &mut Vec<ZoneCrossing>) {
        out.clear();
    }

    fn is_planar(&self) -> bool {
        false
    }

    fn is_uniform_flat(&self) -> bool {
        false
    }

    fn maximum_grade(&self) -> Real {
        Real::INFINITY
    }
}

/// Deterministic air queries. Time is the round clock, including time since impact.
pub trait AirSampler {
    fn sample_air(&self, time_s: Real, position: Vec3) -> AirSample;
}
