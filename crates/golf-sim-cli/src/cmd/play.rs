use clap::{Args, Subcommand};

use super::{Outcome, not_built};

#[derive(Subcommand)]
pub enum Command {
    /// Play one hole: one JSON request per line on stdin, one reply per line on stdout.
    Play(PlayArgs),
}

#[derive(Args)]
pub struct PlayArgs {}

pub fn run(command: Command) -> Outcome {
    match command {
        Command::Play(_) => not_built("play"),
    }
}
