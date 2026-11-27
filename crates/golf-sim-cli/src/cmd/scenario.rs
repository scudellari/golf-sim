use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process,
};

use clap::Subcommand;
use golf_sim::{
    action::{Action, Swing},
    base::{
        Real, Seed, Vec3,
        variant::{self, Array, Components, Dict, FloatWidth, Variant},
    },
    bundle::{HoleBundle, ReproBundle, stroke_count},
    course::course::Course,
    hole::Hole,
    records::hole::{Entry, HoleRecord, decode_envelope},
    setup::{HoleSetup, Mode},
    stroke::{address::Setting, results::End},
};
use serde::{Deserialize, Serialize};

use super::{Outcome, batch::save_repro, replay};
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
    Run {
        path: PathBuf,
        /// Save failing plays in this directory.
        #[arg(long, value_name = "DIR", default_value = "repros")]
        repros: PathBuf,
    },
    /// Write expectations from a run, preserving the action values.
    Bless { file: PathBuf },
    /// Write a regression scenario to stdout.
    FromRepro {
        bundle: PathBuf,
        /// Course bundle to use. Otherwise searches GOLF_COURSES.
        #[arg(long, value_name = "DIR")]
        course: Option<PathBuf>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    course: PathBuf,
    hole: i64,
    #[serde(rename = "day")]
    _day: Day,
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
        ScenarioCommand::Run { path, repros } => {
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
                handle(&file, Some(&repros), &mut out)
                    .map_err(|error| format!("{}: {error}", file.display()))?;
            }
        }
        ScenarioCommand::Bless { file } => {
            handle(&file, None, &mut out)
                .map_err(|error| format!("{}: {error}", file.display()))?;
        }
        ScenarioCommand::FromRepro { bundle, course } => {
            from_repro(&bundle, course.as_deref(), &mut out)?;
        }
    }
    Ok(())
}

fn handle(file: &Path, repros: Option<&Path>, out: &mut impl Write) -> Outcome {
    let source = fs::read_to_string(file)?;
    let mut document: toml::Table = toml::from_str(&source)?;
    let base = file.parent().unwrap_or_else(|| Path::new(""));
    if document.contains_key("record") {
        return run_recorded(file, repros, out, base, document.try_into()?);
    }
    let expectations = document.remove("expected");
    let scenario: Scenario = toml::Value::Table(document.clone()).try_into()?;
    let expected: Option<Expected> = if repros.is_some() {
        Some(
            expectations
                .ok_or("expected: missing expectations. Run golf scenario bless")?
                .try_into()?,
        )
    } else {
        None
    };
    let course = Course::open(&base.join(&scenario.course))?;
    let mut setup = HoleSetup::standard(&course, scenario.hole, Seed(scenario.seed));
    setup.settings.info_mode = match scenario.mode {
        PlayMode::Caddie => Mode::Caddie,
        PlayMode::Tour => Mode::Tour,
    };
    let record = HoleRecord::begin(setup, HoleRecord::agent_driver("golf scenario"));
    let mut hole = match Hole::resume(course.clone(), &record.to_bytes()?) {
        Ok(hole) => hole.with_traces(3),
        Err(error) => {
            if let Some(root) = repros {
                report_repro(
                    root,
                    file,
                    HoleBundle::from_record(&record, error.to_string(), 0),
                );
            }
            return Err(error.into());
        }
    };
    let mut focus = None;
    let mut original = None;
    let result = scenario.play(&course, &mut hole).and_then(|actual| {
        if let Some(expected) = expected
            && let Err((stroke, error)) = expected.check(&actual, scenario.tolerance_m)
        {
            focus = stroke;
            // Keep the authored baseline as well as the observed record.
            if expected.strokes.len() == actual.strokes.len() {
                let mut baseline = document.clone();
                baseline.remove("course");
                baseline.insert("expected".into(), toml::Value::try_from(&expected)?);
                original = Some(toml::to_string(&baseline)?);
            }
            return Err(error);
        }
        Ok(actual)
    });
    let actual = match result {
        Ok(actual) => actual,
        Err(error) => {
            if let Some(root) = repros {
                let focus = focus.unwrap_or_else(|| stroke_count(hole.record()));
                let mut bundle = hole.bundle(error.to_string(), focus);
                if let Ok(bundle) = &mut bundle
                    && let Some(baseline) = original
                {
                    bundle
                        .manifest
                        .extra
                        .insert("scenario".into(), baseline.into());
                }
                report_repro(root, file, bundle);
            }
            return Err(error);
        }
    };
    if repros.is_none() {
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
        passed(out, file, actual.strokes.len(), actual.score)?;
    }
    Ok(())
}

fn report_repro(
    root: &Path,
    file: &Path,
    bundle: Result<HoleBundle, golf_sim::records::hole::Refusal>,
) {
    let name = file.file_name().unwrap_or_default().to_string_lossy();
    match bundle
        .map_err(Into::into)
        .and_then(|bundle| save_repro(root, &format!("scenario-{name}"), &bundle))
    {
        Ok(path) => eprintln!("saved repro {}", path.display()),
        Err(error) => eprintln!("could not save repro for {}: {error}", file.display()),
    }
}

fn passed(out: &mut impl Write, file: &Path, strokes: usize, score: usize) -> io::Result<()> {
    writeln!(
        out,
        "{}: passed ({strokes} strokes, score {score})",
        file.display()
    )
}

impl Scenario {
    fn play(
        &self,
        course: &Course,
        hole: &mut Hole,
    ) -> Result<Expected, Box<dyn std::error::Error>> {
        if !self.tolerance_m.is_finite() || self.tolerance_m < 0.0 {
            return Err("tolerance_m must be finite and nonnegative".into());
        }
        let mut strokes = Vec::new();
        for (index, action) in self.actions.iter().cloned().enumerate() {
            let action = scenario_action(action, course)
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
                #[cfg(test)]
                let actual = regress(actual);
                strokes.push(actual);
            }
        }
        Ok(Expected {
            strokes,
            score: hole.rules().strokes(),
        })
    }
}

impl Expected {
    fn check(
        &self,
        actual: &Self,
        tolerance_m: Real,
    ) -> Result<(), (Option<usize>, Box<dyn std::error::Error>)> {
        for (index, (wanted, got)) in self.strokes.iter().zip(&actual.strokes).enumerate() {
            wanted
                .check(got, index, tolerance_m)
                .map_err(|error| (Some(index + 1), error))?;
        }
        if self.strokes.len() != actual.strokes.len() {
            let index = self.strokes.len().min(actual.strokes.len());
            let summary = |stroke: Option<&Stroke>| {
                stroke.map_or_else(
                    || "no stroke".into(),
                    |s| format!("{} at {:?}", s.end, s.rest),
                )
            };
            return Err((
                (index < actual.strokes.len()).then_some(index + 1),
                format!(
                    "expected.strokes[{index}]: expected {}, got {}",
                    summary(self.strokes.get(index)),
                    summary(actual.strokes.get(index))
                )
                .into(),
            ));
        }
        if self.score != actual.score {
            return Err((
                None,
                format!(
                    "expected.score: expected {}, got {}",
                    self.score, actual.score
                )
                .into(),
            ));
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordedScenario {
    course: PathBuf,
    record: toml::Table,
    expected: RecordedExpected,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordedExpected {
    score: usize,
}

fn from_repro(bundle: &Path, explicit: Option<&Path>, out: &mut impl Write) -> Outcome {
    let bundle = match ReproBundle::read(bundle)? {
        ReproBundle::Hole(bundle) => bundle,
        ReproBundle::Earlier(bundle) => return Err(replay::earlier(bundle)),
        ReproBundle::Stroke(_) => return Err("a scenario needs a hole bundle".into()),
    };
    let record = bundle.hole_record()?;
    let course_path =
        replay::find_course_path(&bundle.manifest.course, explicit, "Bundle")?.canonicalize()?;
    let course = Course::open(&course_path)?;
    record.check_course(&course)?;
    if let Some(baseline) = bundle
        .manifest
        .extra
        .get("scenario")
        .and_then(serde_json::Value::as_str)
    {
        let mut document: toml::Table = toml::from_str(baseline)?;
        document.insert("course".into(), toml::Value::try_from(&course_path)?);
        let actions = document
            .get_mut("actions")
            .and_then(toml::Value::as_array_mut)
            .ok_or("repro scenario must have actions")?;
        let stored = record.entries.iter().filter_map(|entry| match entry {
            Entry::Stroke(entry) => Some(&entry.swing),
            Entry::Decision(_) => None,
        });
        let swings = actions
            .iter_mut()
            .filter_map(|action| action.get_mut("stroke"));
        let mut stored = stored.peekable();
        for stroke in swings {
            let swing = stored
                .next()
                .and_then(replay::recorded_swing)
                .ok_or("missing recorded swing")?;
            let Swing::Exact {
                power,
                timing_fault,
                strike_m,
            } = swing
            else {
                return Err("the original scenario needs an exact swing".into());
            };
            let mut exact = toml::Table::new();
            exact.insert("power".into(), toml::Value::Float(power));
            exact.insert("timing_fault".into(), toml::Value::Float(timing_fault));
            if let Some(point) = strike_m {
                exact.insert(
                    "strike_m".into(),
                    toml::Value::try_from([point.x, point.y])?,
                );
            }
            stroke
                .as_table_mut()
                .ok_or("expected a stroke table")?
                .insert(
                    "swing".into(),
                    toml::Value::Table(toml::Table::from_iter([(
                        "exact".into(),
                        toml::Value::Table(exact),
                    )])),
                );
        }
        if stored.peek().is_some() {
            return Err("repro scenario is missing recorded strokes".into());
        }
        write!(out, "{}", toml::to_string(&document)?)?;
    } else {
        let hole = Hole::resume(course, &bundle.record)?;
        let record = encode_table(&decode_envelope(&bundle.record)?)?;
        let scenario = RecordedScenario {
            course: course_path,
            record,
            expected: RecordedExpected {
                score: hole.rules().strokes(),
            },
        };
        write!(out, "{}", toml::to_string_pretty(&scenario)?)?;
    }
    Ok(())
}

fn run_recorded(
    file: &Path,
    repros: Option<&Path>,
    out: &mut impl Write,
    base: &Path,
    scenario: RecordedScenario,
) -> Outcome {
    let course = Course::open(&base.join(&scenario.course))?;
    let current = HoleRecord::begin(HoleSetup::standard(&course, 0, Seed(0)), Dict::default());
    let record = Dict::new(
        scenario
            .record
            .into_iter()
            .map(|(key, value)| {
                let value = if key == "versions" {
                    Variant::Dict(current.versions.clone())
                } else {
                    decode_value(value)?
                };
                Ok((Variant::String(key), value))
            })
            .collect::<Result<_, Box<dyn std::error::Error>>>()?,
    );
    let body = variant::encode(&Variant::Dict(record))?;
    let mut bytes = b"LLHOLE01".to_vec();
    bytes.extend_from_slice(&u32::try_from(body.len())?.to_le_bytes());
    bytes.extend_from_slice(&body);
    let record = HoleRecord::from_bytes(&bytes)?;
    let result = Hole::resume(course, &bytes)
        .map_err(Box::<dyn std::error::Error>::from)
        .and_then(|hole| {
            let score = hole.rules().strokes();
            if scenario.expected.score != score {
                return Err(format!(
                    "expected.score: expected {}, got {score}",
                    scenario.expected.score
                )
                .into());
            }
            Ok(score)
        });
    match (result, repros) {
        (Ok(score), Some(_)) => Ok(passed(out, file, stroke_count(&record), score)?),
        (Ok(_), None) => {
            Err("a recorded scenario already contains the recorded expectations".into())
        }
        (Err(error), repros) => {
            if let Some(root) = repros {
                report_repro(
                    root,
                    file,
                    HoleBundle::from_record(&record, error.to_string(), stroke_count(&record)),
                );
            }
            Err(error)
        }
    }
}

fn encode_table(dict: &Dict) -> Result<toml::Table, Box<dyn std::error::Error>> {
    dict.entries()
        .iter()
        .map(|(key, value)| {
            Ok((
                key.as_str()
                    .ok_or("record key must be a string")?
                    .to_owned(),
                encode_value(value)?,
            ))
        })
        .collect()
}

fn encode_value(value: &Variant) -> Result<toml::Value, Box<dyn std::error::Error>> {
    use toml::Value as T;
    // TOML has no vector types or null. The tags distinguish them from arrays and tables.
    let tagged = |key: &str, data| T::Table(toml::Table::from_iter([(format!("${key}"), data)]));
    Ok(match value {
        Variant::Nil => tagged("nil", T::Boolean(true)),
        Variant::Bool(value) => T::Boolean(*value),
        Variant::Int(value) => T::Integer(value.value()),
        Variant::Float(value) => T::Float(value.value()),
        Variant::String(value) => T::String(value.clone()),
        Variant::Vector2(value) => tagged("vec2", T::try_from(value.values)?),
        Variant::Vector3(value) => tagged("vec3", T::try_from(value.values)?),
        Variant::PackedVector2Array(value) => tagged("points2", T::try_from(&value.values)?),
        Variant::PackedVector3Array(value) => tagged("points3", T::try_from(&value.values)?),
        Variant::Dict(dict) => T::Table(encode_table(dict)?),
        Variant::Array(array) => T::Array(
            array
                .items
                .iter()
                .map(encode_value)
                .collect::<Result<_, _>>()?,
        ),
        _ => return Err("unsupported value in hole record".into()),
    })
}

fn decode_value(value: toml::Value) -> Result<Variant, Box<dyn std::error::Error>> {
    use toml::Value as T;
    let width = FloatWidth::Double;
    Ok(match value {
        T::Boolean(value) => Variant::Bool(value),
        T::Integer(value) => value.into(),
        T::Float(value) => value.into(),
        T::String(value) => Variant::String(value),
        T::Array(array) => Variant::Array(Array::new(
            array
                .into_iter()
                .map(decode_value)
                .collect::<Result<_, _>>()?,
        )),
        T::Table(table) => {
            let tag = table
                .iter()
                .next()
                .filter(|(key, _)| table.len() == 1 && key.starts_with('$'));
            match tag.map(|(key, value)| (key.as_str(), value.clone())) {
                Some(("$nil", T::Boolean(true))) => Variant::Nil,
                Some(("$vec2", value)) => Variant::Vector2(Components {
                    values: value.try_into()?,
                    width,
                }),
                Some(("$vec3", value)) => Variant::Vector3(Components {
                    values: value.try_into()?,
                    width,
                }),
                Some(("$points2", value)) => Variant::PackedVector2Array(Components {
                    values: value.try_into()?,
                    width,
                }),
                Some(("$points3", value)) => Variant::PackedVector3Array(Components {
                    values: value.try_into()?,
                    width,
                }),
                Some(_) => return Err("unknown hole record value tag".into()),
                None => Variant::Dict(Dict::new(
                    table
                        .into_iter()
                        .map(|(key, value)| Ok((Variant::String(key), decode_value(value)?)))
                        .collect::<Result<_, Box<dyn std::error::Error>>>()?,
                )),
            }
        }
        T::Datetime(_) => return Err("a hole record cannot contain a date".into()),
    })
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

#[cfg(test)]
thread_local! {
    static PLANT_REST_REGRESSION: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn regress(mut actual: Stroke) -> Stroke {
    if PLANT_REST_REGRESSION.get() {
        actual.rest[0] += 1.0;
    }
    actual
}

#[cfg(test)]
mod tests {
    use std::{
        env,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    #[test]
    fn a_failing_play_becomes_a_regression_scenario() -> Outcome {
        let root = env::temp_dir().join(format!(
            "golf-scenario-regression-{}-{}",
            process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
        ));
        fs::create_dir(&root)?;
        let result = check_regression(&root);
        fs::remove_dir_all(&root)?;
        result
    }

    fn check_regression(root: &Path) -> Outcome {
        let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata");
        let course = data.join("courses/test-course").canonicalize()?;
        let source = fs::read_to_string(data.join("scenarios/hole-1.toml"))?;
        let mut document: toml::Table = toml::from_str(&source)?;
        document.insert("course".into(), toml::Value::try_from(&course)?);
        let original = root.join("original.toml");
        fs::write(&original, toml::to_string(&document)?)?;
        let repros = root.join("repros");
        let mut out = Vec::new();

        PLANT_REST_REGRESSION.set(true);
        let failure = handle(&original, Some(&repros), &mut out).unwrap_err();
        assert!(failure.to_string().contains("expected.strokes[0].rest"));
        let saved = repros.join("scenario-original.toml");
        let ReproBundle::Hole(bundle) = ReproBundle::read(&saved)? else {
            panic!("expected a hole bundle");
        };
        assert_eq!(bundle.manifest.focus.stroke, 1);
        assert_eq!(stroke_count(&bundle.hole_record()?), 4);
        assert_eq!(bundle.traces.len(), 3);
        println!("Planted change failed at expected.strokes[0].rest and saved a bundle.");

        from_repro(&saved, Some(&course), &mut out)?;
        let generated = root.join("generated.toml");
        fs::write(&generated, &out)?;
        assert!(String::from_utf8(out.clone())?.contains("[actions.stroke.swing.exact]"));
        out.clear();

        PLANT_REST_REGRESSION.set(false);
        handle(&generated, Some(&repros), &mut out)?;
        assert!(String::from_utf8(out.clone())?.contains("passed (4 strokes, score 4)"));
        println!("Generated scenario passed (4 strokes, score 4) on the original build.");

        PLANT_REST_REGRESSION.set(true);
        let failure = handle(&generated, Some(&repros), &mut out).unwrap_err();
        assert!(failure.to_string().contains("expected.strokes[0].rest"));
        println!("Generated scenario failed at expected.strokes[0].rest with the planted change.");
        PLANT_REST_REGRESSION.set(false);
        Ok(())
    }
}
