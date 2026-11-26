use clap::{Args, Subcommand};

use super::{Outcome, not_built};

#[derive(Subcommand)]
pub enum Command {
    /// Print the digests of seeded rounds.
    Digests(DigestsArgs),
    /// Write the golden rounds.
    Golden(GoldenArgs),
}

#[derive(Args)]
pub struct DigestsArgs {}

#[derive(Args)]
pub struct GoldenArgs {}

pub fn run(command: Command) -> Outcome {
    match command {
        Command::Digests(_) => not_built("digests"),
        Command::Golden(_) => not_built("golden"),
    }
}
