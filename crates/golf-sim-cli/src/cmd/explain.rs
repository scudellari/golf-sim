use std::{
    fs,
    io::{self, Write},
};

use clap::Subcommand;
use golf_sim::{base::trace::Trace, bundle::ReproBundle, records::hole::HoleRecord};

use super::{Outcome, replay};
use crate::narrate;

#[derive(Subcommand)]
pub enum Command {
    /// Explain a stroke from a hole record or repro bundle.
    Explain(replay::ExplainArgs),
}

pub fn run(command: Command) -> Outcome {
    let Command::Explain(args) = command;
    let explicit = args.course.as_deref();
    let trace = if args.path.is_dir() {
        match ReproBundle::read(&args.path)? {
            ReproBundle::Stroke(bundle) => {
                let (repro, course) = replay::load_stroke(bundle, explicit, args.stroke)?;
                let mut trace = Trace::default();
                repro.replay(&course, &mut trace)?;
                trace
            }
            ReproBundle::Hole(bundle) => {
                let stroke = args
                    .stroke
                    .or_else(|| replay::bundle_focus(&bundle))
                    .ok_or("this bundle contains no strokes")?;
                let (record, course) = replay::load_hole(&bundle, explicit)?;
                replay::explain_hole(record, course, stroke)?
            }
            ReproBundle::Earlier(bundle) => return Err(replay::earlier(bundle)),
        }
    } else {
        let record = HoleRecord::from_bytes(&fs::read(&args.path)?)?;
        let course = replay::record_course(&record, explicit)?;
        replay::explain_hole(record, course, args.stroke.ok_or("supply --stroke N")?)?
    };
    let json = serde_json::to_vec_pretty(&trace)?;
    let mut out = io::stdout().lock();
    narrate::write(&mut out, &trace)?;
    writeln!(out)?;
    out.write_all(&json)?;
    writeln!(out)?;
    Ok(())
}
