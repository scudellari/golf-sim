//! Shared values of the golf-sim core. Every transcendental goes through [`fm`].

pub mod bundle;
pub mod container;
pub mod corpus;
pub mod digest;
pub mod fm;
pub mod geometry;
pub mod local_data;
pub mod log;
pub mod num;
pub mod seed;
pub mod trace;
pub mod types;
pub mod variant;

pub use types::*;

/// The golf-sim commit supplied at build time.
pub const GOLF_SIM_COMMIT: &str = match option_env!("GOLF_SIM_COMMIT") {
    Some(commit) => commit,
    None => "dev",
};
