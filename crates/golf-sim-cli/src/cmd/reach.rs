use clap::{Args, Subcommand};

use super::{Outcome, not_built};

#[derive(Subcommand)]
pub enum Command {
    /// Drive each tee and test each cup.
    Reach(ReachArgs),
}

#[derive(Args)]
pub struct ReachArgs {}

pub fn run(command: Command) -> Outcome {
    match command {
        Command::Reach(_) => not_built("reach"),
    }
}
