//! The `golf` program.

use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod cmd;
mod policy;
mod protocol;

#[derive(Parser)]
#[command(
    name = "golf",
    version,
    about = "Play and check golf-sim rounds from the command line."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Each module under `cmd` owns its subcommands, so a unit that fills one never edits this file.
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
}

fn main() -> ExitCode {
    let outcome = match Cli::parse().command {
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
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("golf: {error}");
            ExitCode::FAILURE
        }
    }
}
