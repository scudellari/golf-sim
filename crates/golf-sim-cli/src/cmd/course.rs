use clap::{Args, Subcommand};

use super::{Outcome, not_built};

#[derive(Subcommand)]
pub enum Command {
    /// Check a course bundle.
    Course(CourseArgs),
}

#[derive(Args)]
pub struct CourseArgs {}

pub fn run(command: Command) -> Outcome {
    match command {
        Command::Course(_) => not_built("course"),
    }
}
