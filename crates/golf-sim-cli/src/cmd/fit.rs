use clap::{Args, Subcommand};

use super::{Outcome, not_built};

#[derive(Subcommand)]
pub enum Command {
    /// Fit the physics to calibration data.
    Fit(FitArgs),
}

#[derive(Args)]
pub struct FitArgs {}

pub fn run(command: Command) -> Outcome {
    match command {
        Command::Fit(_) => not_built("fit"),
    }
}
