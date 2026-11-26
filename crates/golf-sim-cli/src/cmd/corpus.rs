use std::path::PathBuf;

use clap::{Args, Subcommand};
use golf_sim_testkit::tolerance::{self, Fixture, Runner};

use super::Outcome;

const RUNNERS: &[(&str, Runner)] = &[("variant", tolerance::variant_snapshot)];

fn runner(layer: &str) -> Result<Runner, Box<dyn std::error::Error>> {
    RUNNERS
        .iter()
        .find_map(|(name, run)| (*name == layer).then_some(*run))
        .ok_or_else(|| format!("unknown corpus layer {layer:?}").into())
}

#[derive(Subcommand)]
pub enum Command {
    /// Compare the core with recorded reference fixtures.
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
        layer: String,
    },
    /// Report numeric drift and discrete differences in a fixture.
    Drift {
        fixture: PathBuf,
        #[arg(long)]
        layer: Option<String>,
    },
}

pub fn run(command: Command) -> Outcome {
    match command {
        Command::Corpus(args) => match args.action {
            Action::Check { fixture, layer } => {
                print!("{}", tolerance::check(&fixture, &layer, runner(&layer)?)?);
                Ok(())
            }
            Action::Drift { fixture, layer } => {
                let fixture = Fixture::read(&fixture)?;
                let layer = layer.as_deref().unwrap_or_else(|| fixture.layer());
                print!("{}", fixture.drift(layer, runner(layer)?)?);
                Ok(())
            }
        },
    }
}
