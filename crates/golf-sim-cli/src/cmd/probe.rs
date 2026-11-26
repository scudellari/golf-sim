use clap::{Args, Subcommand};

use super::{Outcome, not_built};

#[derive(Subcommand)]
pub enum Command {
    /// Probe shots across a hole.
    Probe(ProbeArgs),
}

#[derive(Args)]
pub struct ProbeArgs {}

pub fn run(command: Command) -> Outcome {
    match command {
        Command::Probe(_) => not_built("probe"),
    }
}
