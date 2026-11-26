use clap::{Args, Subcommand};

use super::{Outcome, not_built};

#[derive(Subcommand)]
pub enum Command {
    /// Map the strategy space of a hole.
    Strategy(StrategyArgs),
}

#[derive(Args)]
pub struct StrategyArgs {}

pub fn run(command: Command) -> Outcome {
    match command {
        Command::Strategy(_) => not_built("strategy"),
    }
}
