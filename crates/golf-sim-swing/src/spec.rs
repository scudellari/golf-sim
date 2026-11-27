//! The sizes and settings for one swing.

use golf_sim_base::{Real, Vec2, num::clampf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StrikeMode {
    #[default]
    Reticle,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwingSpec {
    pub target_power: Real,
    pub power_cap: Real,
    pub meter_rate_per_s: Real,
    pub tempo_ratio: Real,
    pub ring_min_s: Real,
    pub ring_max_s: Real,
    pub window_early_s: Real,
    pub window_late_s: Real,
    pub timeout_after_s: Real,
    pub strike_mode: StrikeMode,
    pub drift_bias_m: Vec2,
    pub drift_start_sd_m: Vec2,
    pub drift_slide_sd_mps: Vec2,
    pub steer_vertical: bool,
    pub downswing_slide_gain: Real,
    pub wobble_m: Vec2,
}

impl Default for SwingSpec {
    fn default() -> Self {
        Self {
            target_power: 1.0,
            power_cap: 1.1,
            meter_rate_per_s: 1.0 / 1.1,
            tempo_ratio: 3.0,
            ring_min_s: 0.20,
            ring_max_s: 0.42,
            window_early_s: 0.033,
            window_late_s: 0.033,
            timeout_after_s: 0.30,
            strike_mode: StrikeMode::Reticle,
            drift_bias_m: Vec2::ZERO,
            drift_start_sd_m: Vec2::new(0.007, 0.0),
            drift_slide_sd_mps: Vec2::new(0.006, 0.0),
            steer_vertical: false,
            downswing_slide_gain: 3.0,
            wobble_m: Vec2::new(0.0012, 0.0),
        }
    }
}

impl SwingSpec {
    pub fn ring_for_backswing(&self, backswing_s: Real) -> Real {
        clampf(
            backswing_s / self.tempo_ratio,
            self.ring_min_s,
            self.ring_max_s,
        )
    }
}
