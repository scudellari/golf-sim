//! Command-line entry point for the `golf` program.

use std::ffi::OsString;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod cmd;
mod logging;
mod narrate;
mod policy;
mod protocol;
mod report;
mod validate_model;

#[derive(Parser)]
#[command(
    name = "golf",
    version = format!("{} ({})", env!("CARGO_PKG_VERSION"), golf_sim::GOLF_SIM_COMMIT),
    about = "Play and check golf-sim rounds from the command line."
)]
struct Cli {
    /// Write JSON log events to this file.
    #[arg(long, global = true, value_name = "FILE")]
    log: Option<std::path::PathBuf>,
    /// Most verbose level to record.
    #[arg(long, global = true, default_value = "info", value_name = "LEVEL")]
    log_level: tracing_subscriber::filter::LevelFilter,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(flatten)]
    Play(cmd::play::Command),
    #[command(flatten)]
    Replay(cmd::replay::Command),
    #[command(flatten)]
    Schema(cmd::schema::Command),
    #[command(flatten)]
    Batch(cmd::batch::Command),
    #[command(flatten)]
    Stroke(cmd::stroke::Command),
    #[command(flatten)]
    Course(cmd::course::Command),
    #[command(flatten)]
    Day(cmd::day::Command),
    #[command(flatten)]
    Digests(cmd::digests::Command),
    #[command(flatten)]
    Probe(cmd::probe::Command),
    #[command(flatten)]
    Reach(cmd::reach::Command),
    #[command(flatten)]
    Bag(cmd::bag::Command),
    #[command(flatten)]
    Tables(cmd::tables::Command),
    #[command(flatten)]
    Strategy(cmd::strategy::Command),
    #[command(flatten)]
    PlayCourse(cmd::play_course::Command),
    #[command(flatten)]
    Fit(cmd::fit::Command),
    #[command(flatten)]
    Corpus(cmd::corpus::Command),
    #[command(flatten)]
    Explain(cmd::explain::Command),
    #[command(flatten)]
    Scenario(cmd::scenario::Command),
    #[command(flatten)]
    Sweep(cmd::sweep::Command),
    #[command(flatten)]
    Validate(cmd::validate::Command),
    #[command(flatten)]
    Score(cmd::score::Command),
}

/// Run `golf` with arguments starting with the program name.
pub fn run(args: impl IntoIterator<Item = impl Into<OsString> + Clone>) -> ExitCode {
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            let _ = error.print();
            return ExitCode::from(error.exit_code() as u8);
        }
    };
    let run_command = || match cli.command {
        Command::Play(c) => cmd::play::run(c),
        Command::Replay(c) => cmd::replay::run(c),
        Command::Schema(c) => cmd::schema::run(c),
        Command::Batch(c) => cmd::batch::run(c),
        Command::Stroke(c) => cmd::stroke::run(c),
        Command::Course(c) => cmd::course::run(c),
        Command::Day(c) => cmd::day::run(c),
        Command::Digests(c) => cmd::digests::run(c),
        Command::Probe(c) => cmd::probe::run(c),
        Command::Reach(c) => cmd::reach::run(c),
        Command::Bag(c) => cmd::bag::run(c),
        Command::Tables(c) => cmd::tables::run(c),
        Command::Strategy(c) => cmd::strategy::run(c),
        Command::PlayCourse(c) => cmd::play_course::run(c),
        Command::Fit(c) => cmd::fit::run(c),
        Command::Corpus(c) => cmd::corpus::run(c),
        Command::Explain(c) => cmd::explain::run(c),
        Command::Scenario(c) => cmd::scenario::run(c),
        Command::Sweep(c) => cmd::sweep::run(c),
        Command::Validate(c) => cmd::validate::run(c),
        Command::Score(c) => cmd::score::run(c),
    };
    let outcome = logging::with_log(cli.log.as_deref(), cli.log_level, run_command);
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("golf: {error}");
            ExitCode::FAILURE
        }
    }
}
