use std::{
    fs,
    io::{self, BufWriter, Write},
    num::NonZeroUsize,
    ops::RangeInclusive,
    panic,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
};

use clap::{Args, Subcommand, ValueEnum};
use golf_sim::{
    base::{Seed, log::tracing},
    course::course::Course,
    hole::Hole,
    records::hole::HoleRecord,
    setup::{HoleSetup, Mode},
};
use serde::Serialize;

use super::Outcome;
use crate::{
    policy::{self, Hands, Played},
    protocol::{ProfileName, write_line},
};

#[derive(Subcommand)]
pub enum Command {
    /// Play independent holes in parallel and write one JSON line per play, in seed order.
    Batch(BatchArgs),
}

#[derive(Args)]
pub struct BatchArgs {
    #[arg(long, value_name = "DIR")]
    course: PathBuf,
    /// Comma-separated hole numbers. Defaults to every hole.
    #[arg(long, value_delimiter = ',')]
    holes: Vec<i64>,
    /// Inclusive range of seeds.
    #[arg(long, value_name = "FIRST-LAST", value_parser = seed_range, allow_hyphen_values = true)]
    seeds: RangeInclusive<i64>,
    #[arg(long, default_value = "1")]
    jobs: NonZeroUsize,
    /// Save a hole record for each play.
    #[arg(long, value_name = "DIR")]
    records: Option<PathBuf>,
    #[arg(long, value_enum, default_value = "caddie")]
    mode: InfoMode,
    /// Swing with the requested power, zero timing fault and a centred strike.
    #[arg(long, conflicts_with = "profile")]
    exact: bool,
    /// Swing with a seeded skill profile. Defaults to exact swings.
    #[arg(long, value_enum)]
    profile: Option<ProfileName>,
}

#[derive(Clone, ValueEnum)]
enum InfoMode {
    Caddie,
    Tour,
}

#[derive(Serialize)]
struct Row {
    seed: i64,
    hole: i64,
    #[serde(flatten)]
    result: PlayResult,
}

#[derive(Serialize)]
#[serde(untagged)]
enum PlayResult {
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
    let Command::Batch(args) = command;
    let course = Course::open(&args.course)?;
    let mut holes = args.holes;
    if holes.is_empty() {
        holes.extend(course.data().holes.iter().map(|hole| hole.number));
    }
    holes.sort_unstable();
    holes.dedup();
    if holes.is_empty() {
        return Err("course has no holes".into());
    }
    let mode = match args.mode {
        InfoMode::Caddie => Mode::Caddie,
        InfoMode::Tour => Mode::Tour,
    };
    let hands = args.profile.map_or(Hands::Exact, Hands::Profile);
    if let Some(path) = &args.records {
        fs::create_dir_all(path)?;
    }
    let play = |seed, hole| {
        let result = panic::catch_unwind(|| {
            play_one(&course, seed, hole, mode, hands, args.records.as_deref())
        });
        let result = match result {
            Ok(Ok(Played { score, holed })) => PlayResult::Played {
                strokes: score.strokes,
                penalties: score.penalties,
                holed,
            },
            Ok(Err(error)) => PlayResult::Failed { error },
            Err(payload) => {
                let message = payload
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| payload.downcast_ref::<&str>().copied())
                    .unwrap_or("unknown panic");
                PlayResult::Failed {
                    error: format!("play panicked: {message}"),
                }
            }
        };
        Row { seed, hole, result }
    };
    let first = i128::from(*args.seeds.start());
    let count = (i128::from(*args.seeds.end()) - first + 1) * holes.len() as i128;
    let workers = args
        .jobs
        .get()
        .min(usize::try_from(count).unwrap_or(usize::MAX));
    let dispatch = tracing::dispatcher::get_default(Clone::clone);
    let mut output = BufWriter::new(io::stdout().lock());
    thread::scope(|scope| -> Outcome {
        let mut rows = Vec::new();
        for worker in 0..workers {
            // Each worker produces every Nth row. A bounded queue keeps the
            // reorder buffer small even when one hole takes much longer.
            let (send, receive) = mpsc::sync_channel(1);
            let dispatch = &dispatch;
            let play = &play;
            let holes = &holes;
            thread::Builder::new().spawn_scoped(scope, move || {
                tracing::dispatcher::with_default(dispatch, || {
                    for index in (worker as i128..count).step_by(workers) {
                        let seed = first + index / holes.len() as i128;
                        let hole = holes[(index % holes.len() as i128) as usize];
                        if send.send(play(seed as i64, hole)).is_err() {
                            break;
                        }
                    }
                });
            })?;
            rows.push(receive);
        }
        for receive in rows
            .iter()
            .cycle()
            .zip(0..count)
            .map(|(receive, _)| receive)
        {
            write_line(&mut output, &receive.recv()?)?;
        }
        Ok(())
    })?;
    output.flush()?;
    Ok(())
}

fn seed_range(text: &str) -> Result<RangeInclusive<i64>, String> {
    let (first, last) = text
        .get(1..)
        .and_then(|tail| tail.find('-'))
        .map(|index| text.split_at(index + 1))
        .ok_or("expected an inclusive seed range FIRST-LAST")?;
    let first: i64 = first.parse().map_err(|_| "invalid first seed")?;
    let last: i64 = last[1..].parse().map_err(|_| "invalid last seed")?;
    if first > last {
        return Err("first seed must be at most the last seed".into());
    }
    Ok(first..=last)
}

fn play_one(
    course: &Arc<Course>,
    seed: i64,
    number: i64,
    mode: Mode,
    hands: Hands,
    records: Option<&Path>,
) -> Result<Played, String> {
    let mut setup = HoleSetup::standard(course, number, Seed(seed));
    setup.settings.info_mode = mode;
    let record = HoleRecord::begin(setup, HoleRecord::agent_driver("golf batch"));
    let bytes = record.to_bytes().map_err(|error| error.to_string())?;
    let mut hole = Hole::resume(course.clone(), &bytes).map_err(|error| error.to_string())?;
    let result = policy::play(&mut hole, mode, hands);
    if let Some(dir) = records {
        let path = dir.join(format!("hole-{number}-seed-{seed}.rec"));
        let bytes = hole
            .record()
            .to_bytes()
            .map_err(|error| error.to_string())?;
        save_record(&path, &bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    result
}

fn save_record(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static SERIAL: AtomicUsize = AtomicUsize::new(0);
    let staged = path.with_file_name(format!(
        ".golf-batch-{}-{}.tmp",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = fs::File::create_new(&staged)?;
    let written = file.write_all(bytes);
    drop(file);
    let saved = written.and_then(|()| fs::rename(&staged, path));
    if saved.is_err() {
        let _ = fs::remove_file(staged);
    }
    saved
}
