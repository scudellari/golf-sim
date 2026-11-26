use clap::{Args, Subcommand};

use super::{Outcome, not_built};

#[derive(Subcommand)]
pub enum Command {
    /// Play one stroke.
    Stroke(StrokeArgs),
}

#[derive(Args)]
pub struct StrokeArgs {}

pub fn run(command: Command) -> Outcome {
    match command {
        Command::Stroke(_) => not_built("stroke"),
    }
}
