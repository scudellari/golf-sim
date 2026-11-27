//! The fixed ground, air and cup a stroke reads.

use golf_sim_base::{
    Real, Seed, Vec3,
    trace::{Phase, Probe, Source},
};
use golf_sim_course::legal::{LegalMap, StanceFootprint};
use golf_sim_physics::{
    facade::{pressure_at_altitude, wind_at_height},
    plane::ConstantAir,
    samplers::{AirSampler, GroundSampler, is_long_grass},
    values::Cup,
};

use crate::{
    grounds::{CourseGround, Ground},
    lies::{AddressLie, BallLie},
};

const GROUND_WIND_HEIGHT_M: Real = 1.0;
const FELT_WIND_HEIGHT_M: Real = 1.7;
const TREE_SHELTER_RATIO: Real = 1.15;
const SHELTER_NONE_ABOVE_M: Real = 35.0;

#[derive(Debug)]
pub struct World<'a> {
    ground: Ground<'a>,
    air: ConstantAir,
    cup_position: Option<Vec3>,
    clock_s: Real,
    practice_lie: Option<BallLie>,
}

impl<'a> World<'a> {
    pub fn on_course(
        ground: CourseGround<'a>,
        air: ConstantAir,
        cup_position: Option<Vec3>,
        clock_s: Real,
    ) -> Self {
        Self {
            ground: Ground::Course(Box::new(ground)),
            air,
            cup_position: cup_position.filter(|point| point.is_finite()),
            clock_s,
            practice_lie: None,
        }
    }

    pub fn practice(
        ground: impl Into<Ground<'a>>,
        air: ConstantAir,
        lie: BallLie,
        cup_position: Option<Vec3>,
        clock_s: Real,
    ) -> Self {
        Self {
            ground: ground.into(),
            air,
            cup_position: cup_position.filter(|point| point.is_finite()),
            clock_s,
            practice_lie: Some(lie),
        }
    }

    pub fn ground(&self) -> &Ground<'a> {
        &self.ground
    }

    pub fn legal(&self) -> Option<&LegalMap<'a>> {
        match (&self.ground, self.practice_lie) {
            (Ground::Course(ground), None) => Some(ground.legal()),
            _ => None,
        }
    }

    pub fn cup(&self, flagstick_in: bool) -> Option<Cup> {
        self.cup_position.map(|position| Cup {
            position,
            flagstick_in,
        })
    }

    pub fn has_cup(&self) -> bool {
        self.cup_position.is_some()
    }

    pub fn cup_position(&self) -> Option<Vec3> {
        self.cup_position
    }

    pub fn clock_s(&self) -> Real {
        self.clock_s
    }

    pub fn station_pressure_pa(altitude_m: Real) -> Real {
        pressure_at_altitude(altitude_m)
    }

    /// Physics samples this air at the round clock plus the address and flight times.
    pub fn air_at_impact(&self, _gust_time_s: Real) -> &ConstantAir {
        &self.air
    }

    pub fn air_mean(&self, spot: Vec3) -> ConstantAir {
        let mut air = self.air.sample_air(self.clock_s, spot);
        air.gust_mps = Vec3::ZERO;
        ConstantAir::new(air)
    }

    pub fn wind_readout(&self, spot: Vec3, gust_time_s: Real) -> WindReadout {
        let time_s = self.clock_s + gust_time_s;
        let air = self.air.sample_air(time_s, spot);
        let mut mean = air;
        mean.gust_mps = Vec3::ZERO;
        let clear = self
            .air
            .sample_air(time_s, spot + Vec3::new(0.0, SHELTER_NONE_ABOVE_M, 0.0));
        WindReadout {
            mean_mps: air.mean_wind_mps,
            gust_mps: air.gust_mps,
            ground_mps: wind_at_height(&mean, GROUND_WIND_HEIGHT_M),
            felt_mps: wind_at_height(&air, FELT_WIND_HEIGHT_M),
            under_trees: clear.mean_wind_mps.length()
                > TREE_SHELTER_RATIO * air.mean_wind_mps.length(),
            temperature_c: air.temperature_c,
            pressure_pa: air.pressure_pa,
            relative_humidity: air.relative_humidity,
        }
    }

    pub fn wind_at_heights(&self, spot: Vec3, gust_time_s: Real, heights_m: &[Real]) -> Vec<Vec3> {
        let air = self.air.sample_air(self.clock_s + gust_time_s, spot);
        heights_m
            .iter()
            .map(|&height| wind_at_height(&air, height))
            .collect()
    }

    pub fn settle<P: Probe>(&self, spot: Vec3, seed: Seed, probe: &mut P) -> BallLie {
        let ground = self.ground.sample_ground(spot.x, spot.z);
        let spot = Vec3::new(spot.x, ground.height_m, spot.z);
        let (lie, draw, sampled_grass) = if let Some(lie) = self.practice_lie {
            (BallLie { spot, ..lie }, None, false)
        } else {
            let (sit, draw) = BallLie::roll_sit(ground.surface, seed);
            let mut lie = BallLie::on(ground.surface, sit, spot);
            let sampled_grass = is_long_grass(ground.surface) && ground.grass_height_m >= 0.0;
            if sampled_grass {
                lie.set_grass(ground.grass_height_m, Some(ground.grass_density));
            }
            (lie, draw, sampled_grass)
        };

        if P::ENABLED {
            probe.begin_phase(Phase::Lie);
            if let Some(draw) = draw {
                probe.value(
                    "sit_draw",
                    draw,
                    "",
                    Source::SeedDraw { name: "lie".into() },
                );
            }
            let (turf, surface_origin, height_source) = if self.practice_lie.is_some() {
                ("practice_lie", "practice_lie", Source::Computed)
            } else {
                (
                    "surface_lie",
                    "ground.surface",
                    Source::Course {
                        name: "ground.height_m".into(),
                    },
                )
            };
            for (name, value, unit, origin) in [
                ("surface", lie.surface as u8 as Real, "", surface_origin),
                ("ball_height_m", lie.ball_height_m, "m", turf),
                (
                    "grass_height_m",
                    lie.grass_height_m,
                    "m",
                    if sampled_grass {
                        "ground.grass_height_m"
                    } else {
                        turf
                    },
                ),
                (
                    "grass_density",
                    lie.grass_density,
                    "",
                    if sampled_grass && ground.grass_density >= 0.0 {
                        "ground.grass_density"
                    } else {
                        turf
                    },
                ),
                ("sand_firmness", lie.sand_firmness, "", turf),
            ] {
                probe.value(
                    name,
                    value,
                    unit,
                    Source::Turf {
                        name: origin.into(),
                    },
                );
            }
            probe.value("sit", lie.sit as u8 as Real, "", Source::Computed);
            probe.value("ground_height_m", ground.height_m, "m", height_source);
        }

        lie
    }

    pub fn address_lie(&self, ball: BallLie, footprint: &StanceFootprint, aim: Vec3) -> AddressLie {
        AddressLie::sample(&self.ground, ball, footprint, aim)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindReadout {
    pub mean_mps: Vec3,
    pub gust_mps: Vec3,
    pub ground_mps: Vec3,
    pub felt_mps: Vec3,
    pub under_trees: bool,
    pub temperature_c: Real,
    pub pressure_pa: Real,
    pub relative_humidity: Real,
}
