use clap::{Args, Subcommand};

use super::{Outcome, not_built};

#[derive(Subcommand)]
pub enum Command {
    /// Generate one day's conditions.
    Day(DayArgs),
    /// Generate a batch of days.
    Days(DaysArgs),
}

#[derive(Args)]
pub struct DayArgs {}

#[derive(Args)]
pub struct DaysArgs {}

pub fn run(command: Command) -> Outcome {
    match command {
        Command::Day(_) => not_built("day"),
        Command::Days(_) => not_built("days"),
    }
}
