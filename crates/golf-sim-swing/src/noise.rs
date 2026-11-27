use std::f64::consts::TAU;

use golf_sim_base::{Real, Seed, Vec2, fm, seed::SeedStream};

use crate::spec::SwingSpec;

#[derive(Debug)]
pub(crate) struct Drift {
    pub start_m: Vec2,
    pub slide_mps: Vec2,
    phase: Vec2,
}

impl Drift {
    pub fn new(spec: &SwingSpec, seed: Seed) -> Self {
        let mut draws = SeedStream::new(seed);
        let start_m = Vec2::new(
            spec.drift_bias_m.x + gaussian(&mut draws) * spec.drift_start_sd_m.x,
            spec.drift_bias_m.y + gaussian(&mut draws) * spec.drift_start_sd_m.y,
        );
        let slide_mps = Vec2::new(
            gaussian(&mut draws) * spec.drift_slide_sd_mps.x,
            gaussian(&mut draws) * spec.drift_slide_sd_mps.y,
        );
        let phase = Vec2::new(draws.next_float() * TAU, draws.next_float() * TAU);
        Self {
            start_m,
            slide_mps,
            phase,
        }
    }

    pub fn wobble(&self, amplitude_m: Vec2, elapsed_s: Real) -> Vec2 {
        Vec2::new(
            amplitude_m.x * fm::sin(TAU * 0.7 * elapsed_s + self.phase.x),
            amplitude_m.y * fm::sin(TAU * 0.9 * elapsed_s + self.phase.y),
        )
    }
}

fn gaussian(draws: &mut SeedStream) -> Real {
    // Each reticle Gaussian consumes two draws, even when its scale is zero.
    let radius = fm::sqrt(-2.0 * fm::log(1.0 - draws.next_float()));
    radius * fm::cos(TAU * draws.next_float())
}
