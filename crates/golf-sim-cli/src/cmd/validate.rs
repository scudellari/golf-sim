use std::{
    fmt::Display,
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
};

use clap::{Args, Subcommand};
use serde::Serialize;

use super::Outcome;
use crate::validate_model;

#[derive(Subcommand)]
pub enum Command {
    /// Check simulated results against measured data.
    Validate(ValidateArgs),
}

#[derive(Args)]
pub struct ValidateArgs {
    /// Write JSON to stdout or to FILE.
    #[arg(long, value_name = "FILE", num_args = 0..=1, default_missing_value = "-")]
    json: Option<PathBuf>,
    /// Read the public validation corpus from DIR.
    #[arg(long, value_name = "DIR")]
    data: Option<PathBuf>,
}

pub fn run(Command::Validate(args): Command) -> Outcome {
    let report = validate_model::report(args.data.as_deref())?;
    print_report(&report, args.json.as_deref())?;
    let failures = validate_model::failures(&report).count();
    if failures != 0 {
        return Err(format!("{failures} validation quantities failed or did not run").into());
    }
    Ok(())
}

pub(super) fn print_report(report: &(impl Display + Serialize), json: Option<&Path>) -> Outcome {
    let Some(path) = json else {
        write!(io::stdout().lock(), "{report}")?;
        return Ok(());
    };
    let mut output: Box<dyn Write> = if path == Path::new("-") {
        Box::new(io::stdout().lock())
    } else {
        Box::new(File::create(path)?)
    };
    serde_json::to_writer_pretty(&mut output, report)?;
    writeln!(output)?;
    Ok(())
}
