//! Spans for simulation logging. Record only values the simulation already has.
//! Logging must never change simulation state, advance a seed stream, or
//! determine an outcome.
//!
//! Enter round, hole, stroke, and phase spans in order when that context is
//! available. The enclosing spans provide the context for events in each phase.

use crate::{ClubId, Seed};

pub use tracing;
use tracing::{Span, debug_span};

pub const ROUND: &str = "round";
pub const HOLE: &str = "hole";
pub const STROKE: &str = "stroke";

pub fn round(seed: Seed) -> Span {
    debug_span!(ROUND, seed = seed.0)
}

pub fn hole(hole_number: u32) -> Span {
    debug_span!(HOLE, hole_number)
}

pub fn stroke(stroke_number: u32, club: ClubId) -> Span {
    debug_span!(STROKE, stroke_number, club = club.name())
}

macro_rules! phase_spans {
    ($($name:ident, $phase:ident, $label:literal);+ $(;)?) => {
        $(
            pub const $name: &str = $label;

            pub fn $phase() -> Span {
                debug_span!($name)
            }
        )+
    };
}

phase_spans! {
    DELIVERY, delivery, "delivery";
    IMPACT, impact, "impact";
    LAUNCH, launch, "launch";
    FLIGHT, flight, "flight";
    BOUNCE, bounce, "bounce";
    SKID, skid, "skid";
    ROLL, roll, "roll";
    CUP, cup, "cup";
    LIE, lie, "lie";
    RULING, ruling, "ruling";
}
