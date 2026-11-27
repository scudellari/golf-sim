//! Samplers and contact readings used when interpreting a stroke.

use std::cell::Cell;

use golf_sim_base::{ClubId, Real, Surface, Vec2, Vec3, trace::NoProbe};
use golf_sim_physics::{
    facade,
    samplers::{AirSample, AirSampler, GroundSample, GroundSampler},
    values::{Contact, Launch, ShotRequest},
};
use golf_sim_swing::execution::Execution;

use crate::tuning as t;

/// A ground sampler for remeasuring contact with only the soil and sand.
pub struct MownGround<'a>(pub &'a dyn GroundSampler);

impl GroundSampler for MownGround<'_> {
    fn sample_ground(&self, x: Real, z: Real) -> GroundSample {
        GroundSample {
            grass_height_m: 0.0,
            grass_density: 0.0,
            ..self.0.sample_ground(x, z)
        }
    }
}

/// Counts the mean wind sampled during a flight at the air's reference height.
pub struct FlightAir<'a> {
    air: &'a dyn AirSampler,
    tally: Cell<(Vec3, u64)>,
}

impl<'a> FlightAir<'a> {
    pub fn new(air: &'a dyn AirSampler) -> Self {
        Self {
            air,
            tally: Cell::new((Vec3::ZERO, 0)),
        }
    }

    pub fn mean_wind_mps(&self) -> Vec3 {
        let (sum, count) = self.tally.get();
        if count == 0 {
            Vec3::ZERO
        } else {
            sum / count as Real
        }
    }
}

impl AirSampler for FlightAir<'_> {
    fn sample_air(&self, time_s: Real, position: Vec3) -> AirSample {
        let sample = self.air.sample_air(time_s, position);
        let (sum, count) = self.tally.get();
        self.tally.set((sum + sample.mean_wind_mps, count + 1));
        sample
    }
}

/// Whether the club took enough ground before the ball to cost the strike.
pub fn fat(
    request: &ShotRequest,
    launch: &Launch,
    air: &dyn AirSampler,
    ground: &dyn GroundSampler,
) -> bool {
    if launch.contact == Contact::Sand || launch.turf_entry_behind_m <= t::FAT_ENTRY_M {
        return false;
    }
    let expected_depth = if request.lie.surface == Surface::Bunker {
        request.lie.ball_height_m.min(0.0)
    } else {
        0.0
    };
    if request.delivery.sole_height_m <= expected_depth - t::FAT_DEPTH_M {
        return true;
    }
    // A lie's grass drag does not count toward the speed loss from entering the soil.
    let on_soil = facade::strike(request, air, &MownGround(ground), &mut NoProbe);
    on_soil.club_speed_at_ball_mps < (1.0 - t::FAT_SPEED_LOSS) * on_soil.club_speed_mps
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrikeQuality {
    Pure,
    Solid,
    Mishit,
}

pub fn quality(club: ClubId, execution: Execution, launch: &Launch, fat: bool) -> StrikeQuality {
    let best_up = t::club(club).strike_up_mm / 1000.0;
    let miss = Vec2::new(launch.strike_toe_m, launch.strike_up_m - best_up).length();
    if execution.timed_out
        || fat
        || !matches!(launch.contact, Contact::Face | Contact::Sand)
        || miss > t::MISHIT_RADIUS_M
    {
        StrikeQuality::Mishit
    } else if miss <= t::PURE_RADIUS_M
        && execution.timing_fault.abs() <= t::PURE_FAULT
        && launch.interference <= t::PURE_INTERFERENCE
    {
        StrikeQuality::Pure
    } else {
        StrikeQuality::Solid
    }
}
