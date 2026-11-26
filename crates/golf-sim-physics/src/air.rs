//! Air properties and the wind profile used by the solver.

use golf_sim_base::{Real, Vec3, fm, num::clampf};

use crate::{samplers::AirSample, tunables as t};

const MIN_ROUGHNESS_M: Real = 1e-4;

pub fn valid(air: &AirSample) -> bool {
    if !air.temperature_c.is_finite()
        || air.temperature_c <= -t::KELVIN
        || !air.pressure_pa.is_finite()
        || air.pressure_pa < 0.0
        || !air.relative_humidity.is_finite()
        || !air.mean_wind_mps.is_finite()
        || !air.gust_mps.is_finite()
    {
        return false;
    }
    if air.mean_wind_mps + air.gust_mps == Vec3::ZERO {
        return true;
    }
    air.wind_reference_height_m.is_finite()
        && air.roughness_length_m.is_finite()
        && air.wind_reference_height_m >= t::MIN_WIND_REFERENCE_RATIO * roughness(air)
}

fn roughness(air: &AirSample) -> Real {
    air.roughness_length_m.max(MIN_ROUGHNESS_M)
}

pub fn saturation_vapor_pressure_pa(temperature_c: Real) -> Real {
    // Below Buck's fit range, the vapour pressure is negligible.
    if temperature_c <= -100.0 {
        return 0.0;
    }
    t::BUCK_A_PA
        * fm::exp(
            (t::BUCK_B - temperature_c / t::BUCK_D) * (temperature_c / (t::BUCK_C + temperature_c)),
        )
}

/// Density of an ideal mixture of dry air and water vapour, in kg/m³.
pub fn density(air: &AirSample) -> Real {
    let kelvin = air.temperature_c + t::KELVIN;
    let vapor_pressure = air.pressure_pa.min(
        clampf(air.relative_humidity, 0.0, 1.0) * saturation_vapor_pressure_pa(air.temperature_c),
    );
    let dry_pressure = air.pressure_pa - vapor_pressure;
    dry_pressure / (t::R_DRY * kelvin) + vapor_pressure / (t::R_VAPOR * kelvin)
}

/// Dynamic viscosity in Pa·s.
pub fn viscosity(air: &AirSample) -> Real {
    let kelvin = air.temperature_c + t::KELVIN;
    t::SUTHERLAND_MU0
        * fm::pow(kelvin / t::SUTHERLAND_T0, 1.5)
        * (t::SUTHERLAND_T0 + t::SUTHERLAND_S)
        / (kelvin + t::SUTHERLAND_S)
}

/// Wind scaled by the logarithmic profile at the given height above ground.
pub fn wind_at(air: &AirSample, height_m: Real) -> Vec3 {
    let z0 = roughness(air);
    let wind = air.mean_wind_mps + air.gust_mps;
    if wind == Vec3::ZERO
        || height_m <= z0
        || air.wind_reference_height_m < t::MIN_WIND_REFERENCE_RATIO * z0
    {
        return Vec3::ZERO;
    }
    wind * (fm::log(height_m / z0) / fm::log(air.wind_reference_height_m / z0))
}

/// ISA station pressure at the given altitude.
pub fn isa_pressure_pa(altitude_m: Real) -> Real {
    t::ISA_P0 * fm::pow(1.0 - t::ISA_LAPSE * altitude_m / t::ISA_T0, t::ISA_EXPONENT)
}

/// The local air and canopy held constant during one flight step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlightAir {
    pub density_kg_m3: Real,
    pub viscosity_pa_s: Real,
    pub wind_mps: Vec3,
    pub canopy_density: Real,
}

/// Reuses density and viscosity while the thermodynamic inputs stay the same.
#[derive(Debug, Default)]
pub struct Atmosphere {
    cached: Option<Thermodynamics>,
}

#[derive(Debug)]
struct Thermodynamics {
    inputs: [Real; 3],
    density: Real,
    viscosity: Real,
}

impl Atmosphere {
    /// Resolve one sample for the solver. Invalid samples leave the cache untouched.
    pub fn sample(
        &mut self,
        air: &AirSample,
        height_m: Real,
        canopy_density: Real,
    ) -> Option<FlightAir> {
        if !valid(air) || !canopy_density.is_finite() {
            return None;
        }
        let inputs = [air.temperature_c, air.pressure_pa, air.relative_humidity];
        if self
            .cached
            .as_ref()
            .is_none_or(|cached| cached.inputs != inputs)
        {
            self.cached = Some(Thermodynamics {
                inputs,
                density: density(air),
                viscosity: viscosity(air),
            });
        }
        let cached = self.cached.as_ref().unwrap();
        Some(FlightAir {
            density_kg_m3: cached.density,
            viscosity_pa_s: cached.viscosity,
            wind_mps: wind_at(air, height_m),
            canopy_density,
        })
    }
}
