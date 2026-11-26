use clap::{Args, Subcommand};

use super::{Outcome, not_built};

#[derive(Subcommand)]
pub enum Command {
    /// Explain a stroke from a repro record.
    Explain(ExplainArgs),
}

#[derive(Args)]
pub struct ExplainArgs {}

pub fn run(command: Command) -> Outcome {
    match command {
        Command::Explain(_) => not_built("explain"),
    }
}
