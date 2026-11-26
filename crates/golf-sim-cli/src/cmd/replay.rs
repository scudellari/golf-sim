use clap::{Args, Subcommand};

use super::{Outcome, not_built};

#[derive(Subcommand)]
pub enum Command {
    /// Apply a record and name the first entry that diverges.
    Replay(ReplayArgs),
}

#[derive(Args)]
pub struct ReplayArgs {}

pub fn run(command: Command) -> Outcome {
    match command {
        Command::Replay(_) => not_built("replay"),
    }
}
