use clap::{Args, Subcommand};

use super::{Outcome, not_built};

#[derive(Subcommand)]
pub enum Command {
    /// Check reference fixtures, store their twins, and report the drift.
    Corpus(CorpusArgs),
}

#[derive(Args)]
pub struct CorpusArgs {}

pub fn run(command: Command) -> Outcome {
    match command {
        Command::Corpus(_) => not_built("corpus"),
    }
}
