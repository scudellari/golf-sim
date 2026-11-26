use std::io::{self, Write};

use clap::Subcommand;
use golf_sim::base::trace::Trace;

use super::{Outcome, replay};
use crate::narrate;

#[derive(Subcommand)]
pub enum Command {
    /// Explain a stroke from a repro bundle.
    Explain(replay::BundleArgs),
}

pub fn run(command: Command) -> Outcome {
    let Command::Explain(args) = command;
    let (repro, course) = replay::load(&args)?;
    let mut trace = Trace::default();
    repro.replay(&course, &mut trace)?;
    let json = serde_json::to_vec_pretty(&trace)?;
    let mut out = io::stdout().lock();
    narrate::write(&mut out, &trace)?;
    writeln!(out)?;
    out.write_all(&json)?;
    writeln!(out)?;
    Ok(())
}
