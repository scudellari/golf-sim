use clap::{Args, Subcommand};

use super::{Outcome, not_built};

#[derive(Subcommand)]
pub enum Command {
    /// Play rounds in parallel and write one JSON line per round, in seed order.
    Batch(BatchArgs),
}

#[derive(Args)]
pub struct BatchArgs {}

pub fn run(command: Command) -> Outcome {
    match command {
        Command::Batch(_) => not_built("batch"),
    }
}
