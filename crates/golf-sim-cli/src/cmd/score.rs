use std::{fs, path::PathBuf};

use clap::{Args, Subcommand};
use golf_sim_validate::Scorecard;

use super::{Outcome, validate::print_report};
use crate::validate_model;

#[derive(Subcommand)]
pub enum Command {
    /// Score each sub-model by source and role.
    Score(ScoreArgs),
}

#[derive(Args)]
pub struct ScoreArgs {
    /// Write JSON to stdout or to FILE.
    #[arg(long, value_name = "FILE", num_args = 0..=1, default_missing_value = "-")]
    json: Option<PathBuf>,
    /// Read the public validation corpus from DIR.
    #[arg(long, value_name = "DIR")]
    data: Option<PathBuf>,
    /// Compare against a previously saved JSON scorecard.
    #[arg(long, value_name = "BEFORE.json")]
    against: Option<PathBuf>,
}

pub fn run(Command::Score(args): Command) -> Outcome {
    let after = validate_model::report(args.data.as_deref())?.scorecard();
    if let Some(path) = args.against {
        let before = serde_json::from_slice(&fs::read(path)?)?;
        for dataset in &after.skipped_datasets {
            eprintln!("{dataset}");
        }
        print_report(&Scorecard::diff(&before, &after), args.json.as_deref())
    } else {
        print_report(&after, args.json.as_deref())
    }
}
