use clap::{Args, Subcommand};

use super::{Outcome, not_built};

#[derive(Subcommand)]
pub enum Command {
    /// Check explicitly stated scenario results.
    Scenario(ScenarioArgs),
}

#[derive(Args)]
pub struct ScenarioArgs {}

pub fn run(command: Command) -> Outcome {
    match command {
        Command::Scenario(_) => not_built("scenario"),
    }
}
