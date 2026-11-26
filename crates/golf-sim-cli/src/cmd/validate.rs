use clap::{Args, Subcommand};

use super::{Outcome, not_built};

#[derive(Subcommand)]
pub enum Command {
    /// Check simulated results against measured data.
    Validate(ValidateArgs),
}

#[derive(Args)]
pub struct ValidateArgs {}

pub fn run(command: Command) -> Outcome {
    match command {
        Command::Validate(_) => not_built("validate"),
    }
}
