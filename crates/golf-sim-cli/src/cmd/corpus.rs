use std::path::PathBuf;

use clap::{Args, Subcommand};
use golf_sim_testkit::corpus::FixtureRun;

use super::Outcome;

#[derive(Subcommand)]
pub enum Command {
    /// Compare the core with recorded reference fixtures.
    ///
    /// Keep companion fixtures in their corpus directories beside the input layer.
    /// Hole advice and observation use records/holes. Hole records use rules/transitions.
    Corpus(CorpusArgs),
}

#[derive(Args)]
pub struct CorpusArgs {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Check a fixture against a layer's tolerances.
    Check {
        fixture: PathBuf,
        #[arg(long)]
        layer: Option<String>,
        /// Course bundle directory or parent of course bundles. May be repeated.
        #[arg(long)]
        course_dir: Vec<PathBuf>,
    },
    /// Report numeric drift and discrete differences in a fixture.
    Drift {
        fixture: PathBuf,
        #[arg(long)]
        layer: Option<String>,
        /// Course bundle directory or parent of course bundles. May be repeated.
        #[arg(long)]
        course_dir: Vec<PathBuf>,
    },
}

pub fn run(command: Command) -> Outcome {
    match command {
        Command::Corpus(args) => match args.action {
            Action::Check {
                fixture,
                layer,
                course_dir,
            } => {
                print!(
                    "{}",
                    FixtureRun::read(&fixture, layer.as_deref(), &course_dir)?.check()?
                );
                Ok(())
            }
            Action::Drift {
                fixture,
                layer,
                course_dir,
            } => {
                print!(
                    "{}",
                    FixtureRun::read(&fixture, layer.as_deref(), &course_dir)?.drift()?
                );
                Ok(())
            }
        },
    }
}
