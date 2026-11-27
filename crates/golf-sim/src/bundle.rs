//! Hole repros use the same folder container as single-stroke repros.

use std::{
    fs::{self, File},
    io::{self, Write},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use golf_sim_base::{
    Error, Real,
    bundle::{Bundle, BundleCourse, BundleFocus, BundleManifest},
    trace::{Phase, Probe, Source, Trace},
};
use golf_sim_stroke::{
    delivery::DeliveryInput,
    stroke::{self, StrokeInput},
};
// Use the workspace's JSON codec already linked by schemars.
use schemars::_private::serde_json::{self, Value};
use serde::Deserialize;

use crate::{
    hole::Hole,
    records::hole::{Entry, HoleRecord, Refusal, StrokeFacts},
};

const TRACES: &str = "traces.json";
const LOG: &str = "log.jsonl";
const PHYSICS: &str = golf_sim_physics::tunables::VERSION;

#[derive(Debug)]
pub enum ReproBundle {
    Stroke(Bundle),
    Hole(HoleBundle),
    Earlier(EarlierBundle),
}

#[derive(Debug, Deserialize)]
pub struct EarlierBundle {
    pub note: String,
    pub focus: Value,
}

#[derive(Debug)]
pub struct HoleBundle {
    pub manifest: BundleManifest,
    pub record: Vec<u8>,
    pub traces: Vec<Trace>,
    /// Retained JSON log lines supplied by the caller, oldest first.
    pub log_tail: Vec<u8>,
}

impl ReproBundle {
    pub fn read(dir: &Path) -> Result<Self, Error> {
        let path = dir.join("bundle.json");
        let bytes = fs::read(&path).map_err(|error| io_error(&path, error))?;
        let metadata: Value = decode_json(&path, &bytes)?;
        let engine = &metadata["engine"];
        let earlier_engine = match engine {
            Value::String(engine) => engine != "golf-sim",
            Value::Object(_) => engine.get("release").is_none() || engine.get("commit").is_none(),
            Value::Null => metadata.get("focus").is_some() && metadata.get("kind").is_none(),
            _ => false,
        };
        if earlier_engine {
            return Ok(Self::Earlier(decode_json(&path, &bytes)?));
        }
        let bundle = Bundle::read(dir)?;
        match bundle.manifest.extra.get("kind").and_then(Value::as_str) {
            None if !bundle.manifest.extra.contains_key("kind") => Ok(Self::Stroke(bundle)),
            Some("stroke") => Ok(Self::Stroke(bundle)),
            Some("hole") => {
                let read_attachment = |name: &str| {
                    let path = dir.join(name);
                    bundle
                        .manifest
                        .files
                        .iter()
                        .any(|file| file == name)
                        .then(|| fs::read(&path).map_err(|error| io_error(&path, error)))
                        .transpose()
                };
                let traces = read_attachment(TRACES)?
                    .map(|bytes| decode_json(&dir.join(TRACES), &bytes))
                    .transpose()?
                    .unwrap_or_default();
                let log_tail = read_attachment(LOG)?.unwrap_or_default();
                Ok(Self::Hole(HoleBundle {
                    manifest: bundle.manifest,
                    record: bundle.record,
                    traces,
                    log_tail,
                }))
            }
            _ => Err(malformed("bundle.json.kind", "expected stroke or hole")),
        }
    }
}

impl Hole {
    /// Capture the current record and the stroke traces retained by this hole.
    /// A focus of zero denotes a play that recorded no strokes.
    pub fn bundle(&self, note: impl Into<String>, focus: usize) -> Result<HoleBundle, Refusal> {
        let mut bundle = HoleBundle::from_record(self.record(), note, focus)?;
        bundle.traces.extend(self.traces().iter().cloned());
        Ok(bundle)
    }

    /// Trace the recorded stroke from the hole state immediately before it.
    pub fn trace_stroke(&self, facts: &StrokeFacts, trace: &mut Trace) -> Result<(), Refusal> {
        let ball = self
            .ball_lie()
            .ok_or_else(|| malformed("stroke", "no ball at address"))?;
        let world = self.world_at(self.clock_s());
        let club = facts.club_id;
        let input = StrokeInput {
            delivery: DeliveryInput {
                club,
                address: facts.address.clone(),
                lie: world.address_lie(ball, &facts.address.footprint(club), facts.aim),
                attempt_seed: self
                    .record()
                    .setup
                    .seed
                    .derive(&format!("attempt/{}", facts.attempt)),
            },
            aim: facts.aim,
            gust_time_s: facts.gust_time_s,
            flagstick_in: facts.flagstick_in,
        };
        let result = stroke::play(&input, facts.execution, &world, trace);
        let next = self.world_at(facts.clock_s);
        next.settle(
            result.outcome.rest,
            input.delivery.attempt_seed.derive("sit"),
            trace,
        );
        Ok(())
    }
}

/// Add the ruling from the verified hole state immediately after the stroke.
pub fn trace_ruling(hole: &Hole, trace: &mut Trace) {
    if let Some(ruling) = hole.rules().last() {
        trace.begin_phase(Phase::Ruling);
        trace.value("status", ruling.status as u8 as Real, "", Source::Computed);
        trace.value(
            "next",
            ruling.consequence as u8 as Real,
            "",
            Source::Computed,
        );
        trace.value(
            "penalty",
            ruling.penalty_strokes as Real,
            "strokes",
            Source::Computed,
        );
    }
}

impl HoleBundle {
    /// Also captures a header-only record when a play was refused at setup.
    pub fn from_record(
        record: &HoleRecord,
        note: impl Into<String>,
        focus: usize,
    ) -> Result<Self, Refusal> {
        check_focus(record, focus)?;
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let versions = record
            .versions
            .entries()
            .iter()
            .filter_map(|(key, value)| {
                Some((key.as_str()?.to_owned(), Value::from(value.as_str()?)))
            })
            .collect::<serde_json::Map<_, _>>();
        let mut manifest = BundleManifest::new(
            BundleCourse {
                id: record.setup.course_id.clone(),
                revision: record.setup.course_revision.clone(),
                extra: Default::default(),
            },
            BundleFocus {
                hole: Some(record.setup.hole),
                stroke: focus,
                extra: Default::default(),
            },
            time,
            note.into(),
        );
        manifest.extra.insert("kind".into(), Value::from("hole"));
        manifest
            .extra
            .insert("versions".into(), Value::Object(versions));
        Ok(Self {
            manifest,
            record: record.to_bytes()?,
            traces: Vec::new(),
            log_tail: Vec::new(),
        })
    }

    pub fn hole_record(&self) -> Result<HoleRecord, Refusal> {
        let physics = self
            .manifest
            .extra
            .get("versions")
            .and_then(|versions| versions.get("physics"))
            .and_then(Value::as_str)
            .ok_or_else(|| Error::WrongType {
                path: "bundle.json.versions.physics".into(),
                expected: "a physics version string",
            })?;
        if physics != PHYSICS {
            return Err(Refusal::IncompatibleVersion {
                model: "physics",
                recorded: physics.into(),
                current: PHYSICS,
            });
        }
        let record = HoleRecord::from_bytes(&self.record)?;
        let setup = &record.setup;
        if self.manifest.course.id != setup.course_id
            || self.manifest.course.revision != setup.course_revision
        {
            return Err(malformed("bundle.json.course", "does not match record.bin").into());
        }
        if self.manifest.focus.hole != Some(setup.hole) {
            return Err(malformed("bundle.json.focus.hole", "does not match record.bin").into());
        }
        check_focus(&record, self.manifest.focus.stroke)?;
        Ok(record)
    }

    /// Write into a new folder or one holding feedback attachments.
    pub fn write(&self, dir: &Path) -> Result<(), Error> {
        let mut manifest = self.manifest.clone();
        let traces_path = dir.join(TRACES);
        let traces = if self.traces.is_empty() && !manifest.files.iter().any(|file| file == TRACES)
        {
            None
        } else {
            Some(
                serde_json::to_vec(&self.traces)
                    .map_err(|error| malformed_path(&traces_path, error))?,
            )
        };
        let log_path = dir.join(LOG);
        let log = (!self.log_tail.is_empty() || manifest.files.iter().any(|file| file == LOG))
            .then_some(self.log_tail.as_slice());
        fs::create_dir_all(dir).map_err(|error| io_error(dir, error))?;
        let mut added = Vec::new();
        let result = (|| {
            for (name, path, contents) in [
                (TRACES, traces_path, traces.as_deref()),
                (LOG, log_path, log),
            ] {
                if let Some(contents) = contents {
                    let mut file =
                        File::create_new(&path).map_err(|error| io_error(&path, error))?;
                    let written = file
                        .write_all(contents)
                        .map_err(|error| io_error(&path, error));
                    added.push(path);
                    written?;
                    if !manifest.files.iter().any(|file| file == name) {
                        manifest.files.push(name.into());
                    }
                }
            }
            Bundle {
                manifest,
                record: self.record.clone(),
            }
            .write(dir)
        })();
        if result.is_err() {
            for path in added {
                let _ = fs::remove_file(path);
            }
        }
        result
    }
}

pub fn stroke_count(record: &HoleRecord) -> usize {
    record
        .entries
        .iter()
        .filter(|entry| matches!(entry, Entry::Stroke(_)))
        .count()
}

fn check_focus(record: &HoleRecord, focus: usize) -> Result<(), Error> {
    let count = stroke_count(record);
    if focus <= count && (focus != 0 || count == 0) {
        return Ok(());
    }
    Err(malformed(
        "bundle.json.focus.stroke",
        format!("stroke {focus} is unavailable"),
    ))
}

fn decode_json<'a, T: Deserialize<'a>>(path: &Path, bytes: &'a [u8]) -> Result<T, Error> {
    serde_json::from_slice(bytes).map_err(|error| malformed_path(path, error))
}

fn malformed(path: &str, message: impl Into<String>) -> Error {
    Error::Malformed {
        path: path.into(),
        message: message.into(),
    }
}

fn malformed_path(path: &Path, error: serde_json::Error) -> Error {
    malformed(&path.display().to_string(), error.to_string())
}

fn io_error(path: &Path, error: io::Error) -> Error {
    Error::Io {
        file: path.display().to_string(),
        message: error.to_string(),
    }
}
