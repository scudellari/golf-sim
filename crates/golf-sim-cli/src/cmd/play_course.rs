use clap::{Args, Subcommand};

use super::{Outcome, not_built};

#[derive(Subcommand)]
pub enum Command {
    /// Play every hole of a course with the built-in policy.
    PlayCourse(PlayCourseArgs),
}

#[derive(Args)]
pub struct PlayCourseArgs {}

pub fn run(command: Command) -> Outcome {
    match command {
        Command::PlayCourse(_) => not_built("play-course"),
    }
}
