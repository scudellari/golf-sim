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
        let before = read_scorecard(&fs::read(path)?)?;
        for dataset in &after.skipped_datasets {
            eprintln!("{dataset}");
        }
        print_report(&Scorecard::diff(&before, &after), args.json.as_deref())
    } else {
        print_report(&after, args.json.as_deref())
    }
}

fn read_scorecard(bytes: &[u8]) -> serde_json::Result<Scorecard> {
    use serde_json::Value;

    let mut scorecard: Value = serde_json::from_slice(bytes)?;
    // Older scorecards used this spelling. Ledger parsing stays strict.
    fn update_verdict(verdict: &mut Value) {
        if verdict.as_str() == Some("local") {
            *verdict = Value::from("local");
        }
    }
    if let Some(rows) = scorecard.get_mut("rows").and_then(Value::as_array_mut) {
        for datasets in rows
            .iter_mut()
            .filter_map(|row| row.get_mut("datasets")?.as_object_mut())
        {
            for verdict in datasets.values_mut() {
                update_verdict(verdict);
            }
        }
    }
    if let Some(skipped) = scorecard
        .get_mut("skipped_datasets")
        .and_then(Value::as_array_mut)
    {
        for verdict in skipped
            .iter_mut()
            .filter_map(|entry| entry.get_mut("verdict"))
        {
            update_verdict(verdict);
        }
    }
    serde_json::from_value(scorecard)
}
