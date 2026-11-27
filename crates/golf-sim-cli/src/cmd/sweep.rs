use std::{
    env,
    ffi::OsString,
    fs,
    io::{self, BufWriter, Write},
    num::NonZeroUsize,
    path::PathBuf,
    process,
};

use clap::{Args, Subcommand, ValueEnum};
use golf_sim::{course::course::Course, records::hole::HoleRecord};
use serde::Deserialize;

use super::Outcome;
use crate::{
    protocol::ProfileName,
    report::{Play, PlayError, Report},
};

#[derive(Subcommand)]
pub enum Command {
    /// Play a balance sweep and report scores and stroke finishes.
    Sweep(SweepArgs),
}

#[derive(Args)]
pub struct SweepArgs {
    #[arg(long, value_name = "DIR")]
    course: PathBuf,
    /// Comma-separated hole numbers. Defaults to every hole.
    #[arg(long, value_delimiter = ',', allow_negative_numbers = true)]
    holes: Vec<i64>,
    /// A constant day or an inclusive range of day seeds.
    #[arg(long, default_value = "constant", allow_hyphen_values = true)]
    days: String,
    /// Comma-separated skill profiles.
    #[arg(
        long,
        value_enum,
        value_delimiter = ',',
        default_value = "scratch,hcp15"
    )]
    profiles: Vec<ProfileName>,
    /// Inclusive range of play seeds.
    #[arg(long, value_name = "FIRST-LAST", allow_hyphen_values = true)]
    seeds: String,
    #[arg(long, default_value = "1")]
    jobs: NonZeroUsize,
    /// Write the report as JSON.
    #[arg(long)]
    json: bool,
}

struct Day {
    name: String,
    batch_args: Vec<OsString>,
}

fn days(value: &str) -> Result<Vec<Day>, String> {
    if value == "constant" {
        return Ok(vec![Day {
            name: value.into(),
            batch_args: Vec::new(),
        }]);
    }
    Err(format!(
        "unsupported --days {value:?}. Use 'constant'. Day seed ranges require the day generator"
    ))
}

#[derive(Deserialize)]
struct BatchRow {
    seed: i64,
    hole: i64,
    #[serde(flatten)]
    result: BatchResult,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum BatchResult {
    Played {
        strokes: usize,
        penalties: usize,
        holed: bool,
    },
    Failed {
        error: String,
    },
}

pub fn run(command: Command) -> Outcome {
    let Command::Sweep(args) = command;
    let days = days(&args.days)?;
    let course = Course::open(&args.course)?;
    let mut profiles: Vec<_> = args
        .profiles
        .iter()
        .map(|profile| profile.to_possible_value().expect("named profile"))
        .collect();
    profiles.sort_unstable_by(|left, right| left.get_name().cmp(right.get_name()));
    profiles.dedup_by(|left, right| left.get_name() == right.get_name());

    let scratch = Scratch::create()?;
    let mut plays = Vec::new();
    let mut errors = Vec::new();
    for (index, day) in days.iter().enumerate() {
        for profile in &profiles {
            let name = profile.get_name();
            let records = scratch.0.join(format!("{index}-{name}"));
            // The batch command owns scheduling and policy. Its records carry the
            // finishes without running the simulation again to recover them.
            let mut batch = process::Command::new(env::current_exe()?);
            batch
                .arg("batch")
                .arg("--course")
                .arg(&args.course)
                .arg(format!("--seeds={}", args.seeds))
                .arg("--jobs")
                .arg(args.jobs.to_string())
                .arg("--profile")
                .arg(name)
                .arg("--records")
                .arg(&records)
                .args(&day.batch_args);
            if !args.holes.is_empty() {
                let holes: Vec<_> = args.holes.iter().map(i64::to_string).collect();
                batch.arg(format!("--holes={}", holes.join(",")));
            }

            let output = batch.output()?;
            if !output.status.success() {
                return Err(format!(
                    "batch failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                )
                .into());
            }
            for row in serde_json::Deserializer::from_slice(&output.stdout).into_iter::<BatchRow>()
            {
                let BatchRow { seed, hole, result } = row?;
                match collect_play(&course, &records, &day.name, name, seed, hole, result) {
                    Ok(play) => plays.push(play),
                    Err(error) => errors.push(PlayError {
                        hole,
                        day: day.name.clone(),
                        profile: name.into(),
                        seed,
                        error,
                    }),
                }
            }
            fs::remove_dir_all(records)?;
        }
    }

    let report = Report::from_plays(plays, errors);
    let mut output = BufWriter::new(io::stdout().lock());
    if args.json {
        serde_json::to_writer(&mut output, &report)?;
        writeln!(output)?;
    } else {
        report.write_text(&mut output)?;
    }
    output.flush()?;
    Ok(())
}

fn collect_play(
    course: &Course,
    records: &std::path::Path,
    day: &str,
    profile: &str,
    seed: i64,
    hole: i64,
    result: BatchResult,
) -> Result<Play, String> {
    let (strokes, penalties, holed) = match result {
        BatchResult::Played {
            strokes,
            penalties,
            holed,
        } => (strokes, penalties, holed),
        BatchResult::Failed { error } => return Err(error),
    };
    let path = records.join(format!("hole-{hole}-seed-{seed}.rec"));
    let bytes = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let record = HoleRecord::from_bytes(&bytes).map_err(|error| error.to_string())?;
    Play::from_record(course, &record, day, profile, strokes, penalties, holed)
        .map_err(|error| error.to_string())
}

struct Scratch(PathBuf);

impl Scratch {
    fn create() -> io::Result<Self> {
        for index in 0.. {
            let path = env::temp_dir().join(format!("golf-sweep-{}-{index}", process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        unreachable!()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
