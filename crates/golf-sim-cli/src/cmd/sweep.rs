use clap::{Args, Subcommand};

use super::{Outcome, not_built};

#[derive(Subcommand)]
pub enum Command {
    /// Run a balance sweep.
    Sweep(SweepArgs),
}

#[derive(Args)]
pub struct SweepArgs {}

pub fn run(command: Command) -> Outcome {
    match command {
        Command::Sweep(_) => not_built("sweep"),
    }
}
