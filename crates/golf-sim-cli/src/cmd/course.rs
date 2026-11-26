use std::{
    io::{self, Write},
    path::PathBuf,
};

use clap::{Args, Subcommand};
use golf_sim::course::course::Course;

use super::Outcome;

#[derive(Subcommand)]
pub enum Command {
    /// Inspect a course bundle.
    Course(CourseArgs),
}

#[derive(Args)]
pub struct CourseArgs {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Open and validate a course bundle.
    Check { dir: PathBuf },
}

pub fn run(command: Command) -> Outcome {
    match command {
        Command::Course(CourseArgs {
            action: Action::Check { dir },
        }) => {
            let course =
                Course::open(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
            writeln!(
                io::stdout().lock(),
                "{} {} {} holes",
                course.data().id,
                course.revision(),
                course.data().holes.len()
            )?;
            Ok(())
        }
    }
}
