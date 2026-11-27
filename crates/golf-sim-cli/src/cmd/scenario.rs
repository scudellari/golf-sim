use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process,
};

use clap::Subcommand;
use golf_sim::{
    action::{Action, Swing},
    base::{Real, Seed, Vec3},
    course::course::Course,
    hole::Hole,
    records::hole::Entry,
    setup::{HoleSetup, Mode},
    stroke::{address::Setting, results::End},
};
use serde::{Deserialize, Serialize};

use super::Outcome;
use crate::{protocol::Decision, report::lie_name};

#[derive(Subcommand)]
pub enum Command {
    /// Run or bless a TOML scenario with exact swings.
    Scenario {
        #[command(subcommand)]
        command: ScenarioCommand,
    },
}

#[derive(Subcommand)]
pub enum ScenarioCommand {
    /// Check the expectations in a file or in each .toml file in a folder.
    Run { path: PathBuf },
    /// Write expectations from a run, preserving the action values.
    Bless { file: PathBuf },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    course: PathBuf,
    hole: i64,
    day: Day,
    seed: i64,
    mode: PlayMode,
    tolerance_m: Real,
    actions: Vec<toml::Table>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Day {
    Constant,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum PlayMode {
    Caddie,
    Tour,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Expected {
    score: usize,
    strokes: Vec<Stroke>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Stroke {
    end: String,
    lie: Lie,
    rest: [Real; 3],
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Lie {
    surface: String,
    sit: String,
}

pub fn run(command: Command) -> Outcome {
    let Command::Scenario { command } = command;
    let mut out = io::stdout().lock();
    match command {
        ScenarioCommand::Run { path } => {
            let files = if path.metadata()?.is_dir() {
                let files = fs::read_dir(&path)?
                    .map(|entry| entry.map(|entry| entry.path()))
                    .collect::<io::Result<Vec<_>>>()?;
                let mut files: Vec<_> = files
                    .into_iter()
                    .filter(|path| {
                        path.extension().is_some_and(|ext| ext == "toml") && path.is_file()
                    })
                    .collect();
                files.sort();
                if files.is_empty() {
                    return Err(format!("{} contains no TOML scenarios", path.display()).into());
                }
                files
            } else {
                vec![path]
            };
            for file in files {
                handle(&file, false, &mut out)
                    .map_err(|error| format!("{}: {error}", file.display()))?;
            }
        }
        ScenarioCommand::Bless { file } => {
            handle(&file, true, &mut out)
                .map_err(|error| format!("{}: {error}", file.display()))?;
        }
    }
    Ok(())
}

fn handle(file: &Path, bless: bool, out: &mut impl Write) -> Outcome {
    let source = fs::read_to_string(file)?;
    let mut document: toml::Table = toml::from_str(&source)?;
    let expectations = document.remove("expected");
    let scenario: Scenario = toml::Value::Table(document.clone()).try_into()?;
    let expected = if bless {
        None
    } else {
        Some(
            expectations
                .ok_or("expected: missing expectations. Run golf scenario bless")?
                .try_into()?,
        )
    };
    let base = file.parent().unwrap_or_else(|| Path::new(""));
    let actual = scenario.play(base, expected.as_ref())?;
    if bless {
        document.insert("expected".into(), toml::Value::try_from(&actual)?);
        replace(file, &toml::to_string(&document)?)?;
        writeln!(
            out,
            "{}: blessed {} strokes, score {}",
            file.display(),
            actual.strokes.len(),
            actual.score
        )?;
    } else {
        writeln!(
            out,
            "{}: passed ({} strokes, score {})",
            file.display(),
            actual.strokes.len(),
            actual.score
        )?;
    }
    Ok(())
}

impl Scenario {
    fn play(
        self,
        base: &Path,
        expected: Option<&Expected>,
    ) -> Result<Expected, Box<dyn std::error::Error>> {
        let Self {
            course,
            hole,
            day: Day::Constant,
            seed,
            mode,
            tolerance_m,
            actions,
        } = self;
        if !tolerance_m.is_finite() || tolerance_m < 0.0 {
            return Err("tolerance_m must be finite and nonnegative".into());
        }
        let course = Course::open(&base.join(course))?;
        let mut setup = HoleSetup::standard(&course, hole, Seed(seed));
        setup.settings.info_mode = match mode {
            PlayMode::Caddie => Mode::Caddie,
            PlayMode::Tour => Mode::Tour,
        };
        let mut hole = Hole::start(course.clone(), setup)?;
        let mut strokes = Vec::new();
        for (index, action) in actions.into_iter().enumerate() {
            let action = scenario_action(action, &course)
                .map_err(|error| format!("actions[{index}]: {error}"))?;
            let step = hole
                .act(action)
                .map_err(|error| format!("actions[{index}]: {error}"))?;
            if let Some(result) = step.stroke {
                let Some(Entry::Stroke(entry)) = hole.record().entries.last() else {
                    unreachable!("a stroke result has a record entry");
                };
                let rest = result.outcome.rest;
                let actual = Stroke {
                    end: end_name(result.outcome.end).into(),
                    lie: Lie {
                        surface: entry.facts.lie.surface.name().to_ascii_lowercase(),
                        sit: lie_name(entry.facts.lie.sit).into(),
                    },
                    rest: [rest.x, rest.y, rest.z],
                };
                if let Some(expected) = expected {
                    let index = strokes.len();
                    let wanted = expected.strokes.get(index).ok_or_else(|| {
                        format!(
                            "expected.strokes[{index}]: expected no stroke, got {} at {:?}",
                            actual.end, actual.rest,
                        )
                    })?;
                    wanted.check(&actual, index, tolerance_m)?;
                }
                strokes.push(actual);
            }
        }
        let score = hole.rules().strokes();
        if let Some(expected) = expected {
            if expected.strokes.len() != strokes.len() {
                let index = strokes.len();
                return Err(format!(
                    "expected.strokes[{index}]: expected {} at {:?}, got no stroke",
                    expected.strokes[index].end, expected.strokes[index].rest,
                )
                .into());
            }
            if expected.score != score {
                return Err(
                    format!("expected.score: expected {}, got {score}", expected.score).into(),
                );
            }
        }
        Ok(Expected { score, strokes })
    }
}

fn scenario_action(
    mut action: toml::Table,
    course: &Course,
) -> Result<Action, Box<dyn std::error::Error>> {
    let address = action
        .get_mut("stroke")
        .and_then(toml::Value::as_table_mut)
        .and_then(|stroke| stroke.remove("address"));
    let decision: Decision = toml::Value::Table(action).try_into()?;
    let mut action = decision
        .action(course, None)
        .map_err(|_| "a stroke must specify an exact swing")?;
    if let Action::Stroke(choice) = &mut action {
        if !matches!(choice.swing, Swing::Exact { .. }) {
            return Err("a stroke must specify an exact swing".into());
        }
        if let Some(address) = address {
            let settings: std::collections::BTreeMap<String, Real> = address.try_into()?;
            for (key, value) in settings {
                let setting = Setting::ALL
                    .into_iter()
                    .find(|setting| setting.key() == key)
                    .ok_or_else(|| format!("unknown address setting {key:?}"))?;
                choice.address = choice.address.clone().with_value(setting, value);
            }
        }
    }
    Ok(action)
}

impl Stroke {
    fn check(&self, actual: &Self, index: usize, tolerance_m: Real) -> Outcome {
        let path = format!("expected.strokes[{index}]");
        for (field, expected, observed) in [
            ("end", &self.end, &actual.end),
            ("lie.surface", &self.lie.surface, &actual.lie.surface),
            ("lie.sit", &self.lie.sit, &actual.lie.sit),
        ] {
            if expected != observed {
                return Err(
                    format!("{path}.{field}: expected {expected:?}, got {observed:?}").into(),
                );
            }
        }
        let [x, y, z] = self.rest;
        let [actual_x, actual_y, actual_z] = actual.rest;
        let distance = Vec3::new(actual_x - x, actual_y - y, actual_z - z).length();
        if !distance.is_finite() || distance > tolerance_m {
            return Err(format!(
                "{path}.rest: expected {:?} within {tolerance_m} m, got {:?} ({distance} m away)",
                self.rest, actual.rest,
            )
            .into());
        }
        Ok(())
    }
}

fn end_name(end: End) -> &'static str {
    match end {
        End::AtRest => "at_rest",
        End::Holed => "holed",
        End::InWater => "in_water",
        End::TimedOut => "timed_out",
    }
}

fn replace(path: &Path, source: &str) -> io::Result<()> {
    let staged = path.with_extension(format!("toml.{}.tmp", process::id()));
    let mut file = fs::File::create_new(&staged)?;
    let written = file.write_all(source.as_bytes());
    drop(file);
    let saved = written.and_then(|()| fs::rename(&staged, path));
    if saved.is_err() {
        let _ = fs::remove_file(staged);
    }
    saved
}
