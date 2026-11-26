use clap::{Args, Subcommand};

use super::{Outcome, not_built};

#[derive(Subcommand)]
pub enum Command {
    /// Print the bag's carries.
    Bag(BagArgs),
}

#[derive(Args)]
pub struct BagArgs {}

pub fn run(command: Command) -> Outcome {
    match command {
        Command::Bag(_) => not_built("bag"),
    }
}
