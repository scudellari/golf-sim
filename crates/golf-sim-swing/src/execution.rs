//! The judgment of a finished swing.

use golf_sim_base::{Real, Vec2};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Execution {
    pub power: Real,
    /// Signed error relative to the ring's close, positive for a late press.
    pub timing_error_s: Real,
    /// Signed error over the window half-width on that side.
    pub timing_fault: Real,
    pub timed_out: bool,
    /// The strike on the club face, when enabled. Positive x is toe and positive y is up.
    pub strike_m: Option<Vec2>,
    pub impact_usec: i64,
    pub backswing_s: Real,
    pub ring_s: Real,
}
