use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use clap::{Args, Subcommand};
use golf_sim::{
    action::{Action, ReliefOption, Swing},
    base::{
        Error, Real, Vec2,
        bundle::BundleCourse,
        container::Manifest,
        trace::{NoProbe, Trace},
        variant::{Dict, Variant},
    },
    bundle::{EarlierBundle, HoleBundle, ReproBundle},
    course::course::Course,
    hole::Hole,
    records::hole::{DecisionKind, Entry, HoleRecord, Refusal, decode_envelope},
    stroke::{
        delivery::DeliveryInput,
        repro::StrokeRepro,
        stroke::{self, StrokeInput},
    },
};

use super::Outcome;
use crate::protocol::ProfileName;

#[derive(Subcommand)]
pub enum Command {
    /// Replay a record or repro bundle and report the first difference.
    Replay(ReplayArgs),
}

#[derive(Args)]
pub struct ReplayArgs {
    #[arg(value_name = "RECORD")]
    path: PathBuf,
    /// Course bundle to use. Otherwise searches GOLF_COURSES.
    #[arg(long, value_name = "DIR")]
    course: Option<PathBuf>,
    /// Stop immediately after this stroke (numbered from 1).
    #[arg(long, value_name = "N")]
    stroke: Option<usize>,
}

#[derive(Args)]
pub struct ExplainArgs {
    #[arg(value_name = "RECORD_OR_BUNDLE")]
    pub(super) path: PathBuf,
    /// Course bundle to use. Otherwise searches GOLF_COURSES.
    #[arg(long, value_name = "DIR")]
    pub(super) course: Option<PathBuf>,
    /// Stroke to replay. A stroke bundle contains only stroke 1.
    #[arg(long, value_name = "N")]
    pub(super) stroke: Option<usize>,
}

pub fn run(command: Command) -> Outcome {
    let Command::Replay(args) = command;
    if !args.path.is_dir() {
        return replay_record(&fs::read(&args.path)?, args.course.as_deref(), args.stroke);
    }
    match ReproBundle::read(&args.path)? {
        ReproBundle::Stroke(bundle) => {
            let (repro, course) = load_stroke(bundle, args.course.as_deref(), args.stroke)?;
            repro.replay(&course, &mut NoProbe)?;
            println!("Stroke 1 replay matched bit for bit.");
            Ok(())
        }
        ReproBundle::Hole(bundle) => {
            let focus = args.stroke.or_else(|| bundle_focus(&bundle));
            let (record, course) = load_hole(&bundle, args.course.as_deref())?;
            replay_hole(record, course, &bundle.record, focus, true)
        }
        ReproBundle::Earlier(bundle) => Err(earlier(bundle)),
    }
}

pub(super) fn replay_record(bytes: &[u8], course: Option<&Path>, stroke: Option<usize>) -> Outcome {
    let record = HoleRecord::from_bytes(bytes)?;
    let course = record_course(&record, course)?;
    replay_hole(record, course, bytes, stroke, false)
}

fn replay_hole(
    mut record: HoleRecord,
    course: Arc<Course>,
    bytes: &[u8],
    stroke: Option<usize>,
    bundle: bool,
) -> Outcome {
    let prefix;
    let bytes = if let Some(stroke) = stroke {
        let end = stroke_index(&record, stroke)?;
        record.entries.truncate(end + 1);
        prefix = record.to_bytes()?;
        &prefix
    } else {
        bytes
    };
    let done = verify(course, record, bytes)?;
    if let Some(stroke) = stroke.filter(|_| bundle) {
        println!("Stroke {stroke} replay matched bit for bit.");
    } else {
        println!(
            "All {} entries replayed bit for bit.",
            done.record().entries.len()
        );
    }
    Ok(())
}

fn verify(
    course: Arc<Course>,
    record: HoleRecord,
    bytes: &[u8],
) -> Result<Hole, Box<dyn std::error::Error>> {
    match Hole::resume(course.clone(), bytes) {
        Ok(hole) => Ok(hole),
        Err(Refusal::Invalid(Error::Diverged { entry, path })) => {
            let values = describe_difference(course, record, bytes, entry, &path);
            Err(format!("entry {entry} diverges at {path}{values}").into())
        }
        Err(error) => Err(error.into()),
    }
}

fn stroke_index(record: &HoleRecord, stroke: usize) -> Result<usize, String> {
    record
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| matches!(entry, Entry::Stroke(_)))
        .nth(stroke.checked_sub(1).ok_or("stroke numbers start at 1")?)
        .map(|(index, _)| index)
        .ok_or_else(|| format!("stroke {stroke} is unavailable in this hole record"))
}

pub(super) fn explain_hole(
    mut record: HoleRecord,
    course: Arc<Course>,
    stroke: usize,
) -> Result<Trace, Box<dyn std::error::Error>> {
    let end = stroke_index(&record, stroke)?;
    record.entries.truncate(end + 1);
    let bytes = record.to_bytes()?;
    let after = verify(course.clone(), record.clone(), &bytes)?;
    let Some(Entry::Stroke(entry)) = record.entries.pop() else {
        unreachable!()
    };
    let before = Hole::resume(course, &record.to_bytes()?)?;
    let mut trace = Trace::default();
    before.trace_stroke(&entry.facts, &mut trace)?;
    golf_sim::bundle::trace_ruling(&after, &mut trace);
    Ok(trace)
}

pub(super) fn bundle_focus(bundle: &HoleBundle) -> Option<usize> {
    Some(bundle.manifest.focus.stroke).filter(|&stroke| stroke != 0)
}

pub(super) fn earlier(bundle: EarlierBundle) -> Box<dyn std::error::Error> {
    format!(
        "bundle from the earlier implementation (note and focus only). Note {:?}, focus {}",
        bundle.note, bundle.focus
    )
    .into()
}

pub(super) fn load_hole(
    bundle: &HoleBundle,
    explicit: Option<&Path>,
) -> Result<(HoleRecord, Arc<Course>), Box<dyn std::error::Error>> {
    let record = bundle.hole_record()?;
    let course = find_course(&bundle.manifest.course, explicit, "Bundle")?;
    Ok((record, course))
}

pub(super) fn record_course(
    record: &HoleRecord,
    explicit: Option<&Path>,
) -> Result<Arc<Course>, Box<dyn std::error::Error>> {
    find_course(
        &BundleCourse {
            id: record.setup.course_id.clone(),
            revision: record.setup.course_revision.clone(),
            extra: Default::default(),
        },
        explicit,
        "Hole record",
    )
}

// Hole::resume identifies the first path. Recompute values only when reporting a failure.
fn describe_difference(
    course: Arc<Course>,
    mut record: HoleRecord,
    bytes: &[u8],
    entry: usize,
    path: &str,
) -> String {
    let stored = entry_value(bytes, entry, path);
    record.entries.truncate(entry + 1);
    let Some(mut bad_entry) = record.entries.pop() else {
        return String::new();
    };
    let Ok(prefix) = record.to_bytes() else {
        return String::new();
    };
    let Ok(mut hole) = Hole::resume(course.clone(), &prefix) else {
        return String::new();
    };
    let computed = match &mut bad_entry {
        Entry::Stroke(stroke) => {
            if !recompute_stroke(&hole, &course, stroke, path) {
                return format!(
                    ": stored {}, computed value unavailable ({:?} prompt)",
                    display(stored),
                    hole.prompt().kind
                );
            }
            record.entries.push(bad_entry);
            record
                .to_bytes()
                .ok()
                .and_then(|bytes| entry_value(&bytes, entry, path))
        }
        Entry::Decision(decision) => {
            let action = match decision.kind {
                DecisionKind::GoForward => Action::GoForward,
                DecisionKind::Drop => Action::Drop(decision.point),
                DecisionKind::Place => Action::Place(decision.point),
                DecisionKind::Relief => {
                    let Some(&option) = usize::try_from(decision.option)
                        .ok()
                        .and_then(|index| RELIEF.get(index))
                    else {
                        return format!(
                            ": stored {}, computed no such relief option",
                            display(stored)
                        );
                    };
                    Action::Relief {
                        option,
                        footprint: decision.footprint.clone(),
                    }
                }
            };
            if let Err(refusal) = hole.act(action) {
                return format!(": stored {}, computed refusal {refusal}", display(stored));
            }
            hole.record()
                .to_bytes()
                .ok()
                .and_then(|bytes| entry_value(&bytes, entry, path))
        }
    };
    format!(
        ": stored {}, computed {}",
        display(stored),
        display(computed)
    )
}

const RELIEF: [ReliefOption; 11] = {
    use ReliefOption::*;
    [
        Cancel,
        StrokeAndDistance,
        BackOnLine,
        Lateral,
        OutsideBunker,
        TemporaryWater,
        GroundUnderRepair,
        ImmovableObstruction,
        Embedded,
        BunkerAbnormalOutside,
        Provisional,
    ]
};

fn recompute_stroke(
    hole: &Hole,
    course: &Course,
    entry: &mut golf_sim::records::hole::StrokeEntry,
    path: &str,
) -> bool {
    let facts = &mut entry.facts;
    if path == "attempt" {
        facts.attempt = hole
            .record()
            .entries
            .iter()
            .filter(|entry| matches!(entry, Entry::Stroke(_)))
            .count() as i64;
        return true;
    }
    if path == "clock_s" {
        facts.clock_s = hole.clock_s() + 150.0;
        return true;
    }
    let setup = &hole.record().setup;
    let Some(ball) = hole.ball_lie() else {
        return false;
    };
    let world = setup.world(course, hole.clock_s()).expect("checked course");
    let seed = setup.seed.derive(&format!("attempt/{}", facts.attempt));
    let input = StrokeInput {
        delivery: DeliveryInput {
            club: facts.club_id,
            address: facts.address.clone(),
            lie: world.address_lie(ball, &facts.address.footprint(facts.club_id), facts.aim),
            attempt_seed: seed,
        },
        aim: facts.aim,
        gust_time_s: facts.gust_time_s,
        flagstick_in: facts.flagstick_in,
    };
    if path.starts_with("execution") {
        let Some(swing) = recorded_swing(&entry.swing) else {
            return false;
        };
        let spec = stroke::swing_spec(&input, setup.settings.assists());
        facts.execution = swing.execute(
            &spec,
            entry.address_usec.expect("checked swing start"),
            seed,
        );
        return true;
    }
    if path == "gust_time_s" {
        let origin = if entry.swing.get("mode").and_then(Variant::as_str) == Some("pressed") {
            entry.address_usec.unwrap_or(0)
        } else {
            0
        };
        facts.gust_time_s = (facts.execution.impact_usec - origin).max(0) as Real / 1_000_000.0;
        return true;
    }
    if !(path.starts_with("rest") || path.starts_with("lie")) {
        return false;
    }
    let result = stroke::play(&input, facts.execution, &world, &mut NoProbe);
    facts.clock_s = hole.clock_s() + 150.0;
    facts.rest = result.outcome.rest;
    let next = setup.world(course, facts.clock_s).expect("checked course");
    facts.lie = next.settle(facts.rest, seed.derive("sit"), &mut NoProbe);
    true
}

pub(super) fn recorded_swing(swing: &Dict) -> Option<Swing> {
    let number = |dict: &Dict, key| match dict.get(key) {
        Some(Variant::Float(value)) => Some(value.value()),
        Some(Variant::Int(value)) => Some(value.value() as Real),
        _ => None,
    };
    match swing.get("mode")?.as_str()? {
        "exact" => {
            let strike_m = match swing.get("strike_m")? {
                Variant::Vector2(strike) => Some(Vec2::new(strike.values[0], strike.values[1])),
                _ => None,
            };
            Some(Swing::Exact {
                power: number(swing, "power")?,
                timing_fault: number(swing, "timing_fault")?,
                strike_m,
            })
        }
        "skill" => {
            let values = swing.get("profile")?.as_dict()?.get("values")?.as_dict()?;
            let Swing::Skill(mut profile) = ProfileName::Scratch.swing() else {
                return None;
            };
            profile.sigma_p_ms = number(values, "sigma_p_ms")?;
            profile.bias_t_ms = number(values, "bias_t_ms")?;
            profile.sigma_t_ms = number(values, "sigma_t_ms")?;
            profile.sigma_s_mm = number(values, "sigma_s_mm")?;
            profile.tau_ms = number(values, "tau_ms")?;
            Some(Swing::Skill(profile))
        }
        _ => None,
    }
}

fn entry_value(bytes: &[u8], index: usize, path: &str) -> Option<Variant> {
    let body = decode_envelope(bytes).ok()?;
    let Variant::Array(entries) = body.get("entries")? else {
        return None;
    };
    let mut value = entries.items.get(index)?;
    if path == "execution.strike_m"
        && matches!(
            value
                .as_dict()?
                .get("execution")?
                .as_dict()?
                .get("has_strike"),
            Some(Variant::Bool(false))
        )
    {
        return Some(Variant::Nil);
    }
    for part in path.split('.') {
        value = match value {
            Variant::Dict(dict) => dict.get(part)?,
            Variant::Vector2(vector) => {
                let axis = ["x", "y"].iter().position(|&axis| axis == part)?;
                return Some(vector.values[axis].into());
            }
            Variant::Vector3(vector) => {
                let axis = ["x", "y", "z"].iter().position(|&axis| axis == part)?;
                return Some(vector.values[axis].into());
            }
            _ => return None,
        };
    }
    Some(value.clone())
}

fn display(value: Option<Variant>) -> String {
    match value {
        Some(Variant::Float(value)) => format!("{:?}", value.value()),
        Some(Variant::Int(value)) => value.value().to_string(),
        Some(Variant::Bool(value)) => value.to_string(),
        Some(Variant::Vector2(vector)) => format!("{:?}", vector.values),
        Some(Variant::Vector3(vector)) => format!("{:?}", vector.values),
        Some(Variant::Nil) => "null".into(),
        Some(value) => format!("{value:?}"),
        None => "<none>".into(),
    }
}

pub(super) fn load_stroke(
    bundle: golf_sim::base::bundle::Bundle,
    explicit: Option<&Path>,
    stroke: Option<usize>,
) -> Result<(StrokeRepro, Arc<Course>), Box<dyn std::error::Error>> {
    let manifest = bundle.manifest;
    let course = find_course(&manifest.course, explicit, "Bundle")?;
    let repro = StrokeRepro::from_bytes(&bundle.record)?;
    if manifest.course.id != repro.setup.course_id {
        return Err("bundle.json and record.bin name different courses".into());
    }
    if manifest.course.revision != repro.setup.course_revision {
        return Err(format!(
            "bundle.json names course revision {}, but record.bin needs {}",
            manifest.course.revision, repro.setup.course_revision
        )
        .into());
    }
    if manifest.focus.hole != Some(repro.setup.hole) || manifest.focus.stroke != 1 {
        return Err("bundle.json focus must name the hole and stroke 1 from record.bin".into());
    }
    if let Some(stroke) = stroke.filter(|&stroke| stroke != 1) {
        return Err(
            format!("stroke {stroke} is unavailable. This bundle contains only stroke 1").into(),
        );
    }
    Ok((repro, course))
}

pub(super) fn find_course_path(
    expected: &BundleCourse,
    explicit: Option<&Path>,
    source: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(dir) = explicit {
        return Ok(dir.to_owned());
    }
    let roots = env::var_os("GOLF_COURSES").map_or_else(
        || vec![PathBuf::from("testdata/courses")],
        |paths| env::split_paths(&paths).collect(),
    );

    let mut wrong_revision = None;
    for root in roots {
        let mut children: Vec<_> = fs::read_dir(&root)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect();
        children.sort();
        for dir in std::iter::once(root).chain(children) {
            let Ok(manifest) = Manifest::read(&dir) else {
                continue;
            };
            if manifest.course_id != expected.id {
                continue;
            }
            if manifest.course_revision == expected.revision {
                return Ok(dir);
            }
            wrong_revision.get_or_insert((dir, manifest.course_revision));
        }
    }
    if let Some((dir, actual)) = wrong_revision {
        return Err(revision_error(&dir, &expected.revision, &actual, source).into());
    }
    Err(format!(
        "course {:?} was not found. Use --course or set GOLF_COURSES",
        expected.id
    )
    .into())
}

fn find_course(
    expected: &BundleCourse,
    explicit: Option<&Path>,
    source: &str,
) -> Result<Arc<Course>, Box<dyn std::error::Error>> {
    open_course(
        &find_course_path(expected, explicit, source)?,
        expected,
        source,
    )
}

fn open_course(
    dir: &Path,
    expected: &BundleCourse,
    source: &str,
) -> Result<Arc<Course>, Box<dyn std::error::Error>> {
    let course = Course::open(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    if course.revision() != expected.revision {
        return Err(revision_error(dir, &expected.revision, course.revision(), source).into());
    }
    if course.data().id != expected.id {
        return Err(format!(
            "{} has course {:?}. The record needs {:?}",
            dir.display(),
            course.data().id,
            expected.id
        )
        .into());
    }
    Ok(course)
}

fn revision_error(dir: &Path, expected: &str, actual: &str, source: &str) -> String {
    format!(
        "course revision mismatch at {}. {source} expects {expected}, found {actual}",
        dir.display()
    )
}
