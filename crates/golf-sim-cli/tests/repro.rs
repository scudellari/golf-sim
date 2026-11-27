use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

use golf_sim::{
    GOLF_SIM_COMMIT,
    base::{
        ClubId,
        bundle::Bundle,
        trace::{Phase, Source, Trace},
        variant::{self, Variant},
    },
    bundle::ReproBundle,
    records::hole::{Entry, Refusal, decode_envelope},
};
use serde_json::{Value, json};

const REVISION: &str = "82b40c2b1d877f0d5a28c48efdd2179e79e4be3ef664b69ec0f7b2e3569072eb";

#[test]
fn drive_chip_and_putt_match_with_and_without_a_trace() {
    for (name, phases, wording) in [
        (
            "drive",
            &[Phase::Flight, Phase::Bounce, Phase::Roll][..],
            "The ball started on the tee",
        ),
        (
            "chip",
            &[Phase::Flight, Phase::Bounce, Phase::Roll][..],
            "The ball bounced on the green",
        ),
        (
            "putt",
            &[Phase::Skid, Phase::Roll, Phase::Cup][..],
            "The ball was holed",
        ),
    ] {
        let path = fixture(name);
        let bundle = Bundle::read(&path).unwrap();
        let manifest_len = fs::metadata(path.join("bundle.json")).unwrap().len();
        assert!(bundle.record.len() as u64 + manifest_len < 100_000);
        assert_eq!(bundle.manifest.course.id, "test-course");
        let replay = golf().arg("replay").arg(&path).output().unwrap();
        assert_eq!(
            success_text(replay),
            "Stroke 1 replay matched bit for bit.\n"
        );

        // Both commands check the entire result against the same saved bytes.
        let explanation = golf()
            .arg("explain")
            .arg(&path)
            .args(["--stroke", "1", "--course"])
            .arg(course())
            .output()
            .unwrap();
        let text = success_text(explanation);
        let (account, json) = text.split_once("\n\n").unwrap();
        let trace: Trace = serde_json::from_str(json).unwrap();
        assert!(account.contains(wording), "{account}");
        for phase in [Phase::Lie, Phase::Delivery, Phase::Impact, Phase::Launch]
            .iter()
            .chain(phases)
        {
            assert!(trace.phases().iter().any(|entry| entry.phase == *phase));
        }
        for kind in [
            "[computed ",
            "[tunable ",
            "[turf ",
            "[course ",
            "[seed draw ",
        ] {
            assert!(account.contains(kind), "missing {kind} in {account}");
        }
        check_number_sources(account, &trace);
    }
}

#[test]
fn stroke_saves_a_bundle_without_changing_stdout() {
    let dir = Scratch::new();
    let saved = dir.0.join("repro");
    let stroke = || {
        let mut command = golf();
        command
            .args(["stroke", "--course"])
            .arg(course())
            .args(["--hole", "2", "--club", "7iron", "--seed", "-304"]);
        command
    };
    let plain = success_text(stroke().output().unwrap());
    let with_repro = success_text(stroke().arg("--repro").arg(&saved).output().unwrap());
    assert_eq!(plain, with_repro);
    let output: Value = serde_json::from_str(&with_repro).unwrap();
    assert_eq!(output["outcome"]["end"], "at_rest");

    let replay = golf()
        .arg("replay")
        .arg(&saved)
        .env("GOLF_COURSES", course())
        .output()
        .unwrap();
    assert_eq!(
        success_text(replay),
        "Stroke 1 replay matched bit for bit.\n"
    );
    let refused = golf()
        .arg("explain")
        .arg(&saved)
        .args(["--stroke", "2"])
        .output()
        .unwrap();
    assert!(failure_text(refused).contains("stroke 2 is unavailable"));
}

#[test]
fn a_mutated_result_byte_is_refused_at_its_first_differing_field() {
    let mut bundle = Bundle::read(&fixture("drive")).unwrap();
    let mut body = variant::decode(&bundle.record[12..]).unwrap();
    let Variant::Dict(dict) = &mut body else {
        panic!("expected the stroke record dictionary");
    };
    let Variant::PackedByteArray(result) = dict.get_mut("result").unwrap() else {
        panic!("expected the stored result bytes");
    };
    let field = variant::encode(&"carry_m".into()).unwrap();
    let index = result
        .windows(field.len())
        .position(|bytes| bytes == field)
        .unwrap();
    result[index + field.len() + 4] ^= 1;
    let mutated = variant::encode(&body).unwrap();
    assert_eq!(
        bundle.record[12..]
            .iter()
            .zip(&mutated)
            .filter(|(before, after)| before != after)
            .count(),
        1
    );
    bundle.record[12..].copy_from_slice(&mutated);
    let dir = Scratch::new();
    bundle.write(&dir.0).unwrap();

    for command in ["replay", "explain"] {
        let refused = golf().arg(command).arg(&dir.0).output().unwrap();
        assert!(failure_text(refused).contains("diverges at result.outcome.carry_m"));
    }
}

#[test]
fn a_different_course_revision_is_refused_with_both_revisions() {
    let mut bundle = Bundle::read(&fixture("chip")).unwrap();
    let other = "0".repeat(64);
    bundle.manifest.course.revision = other.clone();
    let dir = Scratch::new();
    bundle.write(&dir.0).unwrap();

    for command in ["replay", "explain"] {
        let refused = golf().arg(command).arg(&dir.0).output().unwrap();
        let stderr = failure_text(refused);
        assert!(
            stderr.contains(&format!("Bundle expects {other}, found {REVISION}")),
            "{stderr}"
        );
    }
    let refused = golf()
        .arg("replay")
        .arg(&dir.0)
        .arg("--course")
        .arg(course())
        .output()
        .unwrap();
    let stderr = failure_text(refused);
    assert!(
        stderr.contains(&format!("Bundle expects {other}, found {REVISION}")),
        "{stderr}"
    );
}

#[test]
fn unfamiliar_manifest_fields_survive_a_read_and_write() {
    let source = Scratch::new();
    let destination = Scratch::new();
    let original = fixture("putt");
    fs::copy(original.join("record.bin"), source.0.join("record.bin")).unwrap();
    let mut manifest: Value = read_json(&original.join("bundle.json"));
    manifest["future"] = json!({"array": [null, true, "new field", 4.125]});
    manifest["engine"]["build_info"] = json!("unfamiliar");
    manifest["course"]["aliases"] = json!(["another name"]);
    manifest["focus"]["frame"] = json!(42);
    fs::write(
        source.0.join("bundle.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();

    Bundle::read(&source.0)
        .unwrap()
        .write(&destination.0)
        .unwrap();
    assert_eq!(read_json(&destination.0.join("bundle.json")), manifest);
    assert_eq!(
        fs::read(destination.0.join("record.bin")).unwrap(),
        fs::read(original.join("record.bin")).unwrap()
    );
}

#[test]
fn hole_bundle_replays_its_focus_and_explains_the_lip_out() {
    let dir = Scratch::new();
    let path = lip_out_for_build(&dir);
    let ReproBundle::Hole(bundle) = ReproBundle::read(&path).unwrap() else {
        panic!("expected a hole bundle");
    };
    let record = bundle.hole_record().unwrap();
    assert_eq!(record.setup.seed.0, 5);
    assert_eq!(record.setup.hole, 2);
    assert_eq!(bundle.manifest.focus.stroke, 2);
    assert_eq!(record.entries.len(), 3);
    let Entry::Stroke(stroke) = &record.entries[1] else {
        panic!("expected the focus stroke");
    };
    assert_eq!(stroke.facts.club_id, ClubId::Putter);

    for (args, stroke) in [(&[][..], 2), (&["--stroke", "3"][..], 3)] {
        let output = golf().arg("replay").arg(&path).args(args).output().unwrap();
        assert_eq!(
            success_text(output),
            format!("Stroke {stroke} replay matched bit for bit.\n")
        );
    }
    let output = golf()
        .arg("explain")
        .arg(&path)
        .args(["--stroke", "2"])
        .output()
        .unwrap();
    let output = success_text(output);
    let (account, json) = output.split_once("\n\n").unwrap();
    assert!(account.contains("The rolling ball lipped out"), "{account}");
    let trace: Trace = serde_json::from_str(json).unwrap();
    assert_eq!(trace, bundle.traces[0]);
}

#[test]
fn a_hole_bundle_preserves_metadata_and_retained_attachments() {
    let source = Scratch::new();
    let destination = Scratch::new();
    let path = fixture("lip-out");
    let ReproBundle::Hole(mut bundle) = ReproBundle::read(&path).unwrap() else {
        panic!("expected a hole bundle");
    };
    bundle
        .manifest
        .extra
        .insert("future".into(), json!({"settings": [true, 2, "value"]}));
    bundle
        .manifest
        .focus
        .extra
        .insert("camera".into(), json!({"distance": 3.5}));
    bundle
        .manifest
        .engine
        .extra
        .insert("name".into(), json!("other"));
    bundle
        .manifest
        .course
        .extra
        .insert("aliases".into(), json!(["test"]));
    bundle.manifest.files.push("screenshot.png".into());
    bundle.log_tail = b"{\"message\":\"retained event\"}\n".to_vec();
    fs::write(source.0.join("screenshot.png"), b"opaque feedback data").unwrap();
    bundle.write(&source.0).unwrap();

    let ReproBundle::Hole(read) = ReproBundle::read(&source.0).unwrap() else {
        panic!("expected a hole bundle");
    };
    assert_eq!(read.traces, bundle.traces);
    assert_eq!(read.log_tail, bundle.log_tail);
    assert_eq!(read.record, bundle.record);
    read.write(&destination.0).unwrap();
    for name in ["bundle.json", "record.bin", "traces.json", "log.jsonl"] {
        assert_eq!(
            fs::read(source.0.join(name)).unwrap(),
            fs::read(destination.0.join(name)).unwrap(),
            "{name}"
        );
    }
    assert!(!destination.0.join("screenshot.png").exists());
}

#[test]
fn a_hole_bundle_names_both_physics_versions_on_refusal() {
    let destination = Scratch::new();
    let ReproBundle::Hole(mut bundle) = ReproBundle::read(&fixture("lip-out")).unwrap() else {
        panic!("expected a hole bundle");
    };
    bundle.manifest.extra.get_mut("versions").unwrap()["physics"] = json!("99.1");
    let Refusal::IncompatibleVersion {
        model,
        recorded,
        current,
    } = bundle.hole_record().unwrap_err()
    else {
        panic!("expected a typed version refusal");
    };
    assert_eq!(model, "physics");
    assert_eq!(recorded, "99.1");
    assert_eq!(current, golf_sim::physics::tunables::VERSION);
    bundle.write(&destination.0).unwrap();
    let outcome = golf().arg("replay").arg(&destination.0).output().unwrap();
    assert!(failure_text(outcome).contains(&format!(
        "physics version 99.1, and this build has {current}"
    )));
}

#[test]
fn an_earlier_bundle_exposes_its_note_and_focus_without_opening_the_record() {
    let dir = Scratch::new();
    fs::write(
        dir.0.join("bundle.json"),
        serde_json::to_vec(&json!({
            "engine": "gdscript",
            "note": "example note",
            "focus": {"hole": 2, "stroke": 3},
            "files": ["missing-record.bin"]
        }))
        .unwrap(),
    )
    .unwrap();
    let outcome = golf().arg("replay").arg(&dir.0).output().unwrap();
    let text = failure_text(outcome);
    assert!(text.contains("earlier implementation (note and focus only)"));
    assert!(text.contains("example note"));
    assert!(text.contains(r#""stroke":3"#));
}

#[test]
fn from_repro_writes_a_runnable_scenario_with_saved_swings() {
    let dir = Scratch::new();
    let scenario = dir.0.join("generated.toml");
    let bundle = lip_out_for_build(&dir);
    let result = golf()
        .args(["scenario", "from-repro"])
        .arg(&bundle)
        .output()
        .unwrap();
    let source = success_text(result);
    let document: toml::Table = toml::from_str(&source).unwrap();
    let strokes = document["record"]["entries"].as_array().unwrap();
    let ReproBundle::Hole(bundle) = ReproBundle::read(&bundle).unwrap() else {
        panic!("expected a hole bundle");
    };
    let record = bundle.hole_record().unwrap();
    for (entry, stroke) in record.entries.iter().zip(strokes) {
        let Entry::Stroke(entry) = entry else {
            panic!("expected a stroke");
        };
        let Variant::Float(power) = entry.swing.get("power").unwrap() else {
            panic!("expected exact swing power");
        };
        assert_eq!(stroke["swing"]["mode"].as_str(), Some("exact"));
        assert_eq!(
            stroke["swing"]["power"].as_float().unwrap().to_bits(),
            power.value().to_bits()
        );
    }
    fs::write(&scenario, source).unwrap();
    let result = golf()
        .current_dir(&dir.0)
        .args(["scenario", "run"])
        .arg(scenario)
        .output()
        .unwrap();
    assert!(success_text(result).contains("passed (3 strokes, score 3)"));
    assert!(!dir.0.join("repros").exists());
}

#[test]
fn a_refused_batch_play_saves_its_record_under_the_requested_directory() {
    let dir = Scratch::new();
    let repros = dir.0.join("reports");
    let outcome = golf()
        .current_dir(&dir.0)
        .args(["batch", "--course"])
        .arg(course())
        .args(["--holes", "99", "--seeds", "5-5", "--repros"])
        .arg(&repros)
        .output()
        .unwrap();
    let path = repros.join("batch-hole-99-seed-5");
    assert!(
        String::from_utf8_lossy(&outcome.stderr)
            .contains(&format!("saved repro {}", path.display()))
    );
    let row: Value = serde_json::from_str(&success_text(outcome)).unwrap();
    assert!(row["error"].as_str().unwrap().contains("no such tee"));
    let ReproBundle::Hole(bundle) = ReproBundle::read(&path).unwrap() else {
        panic!("expected a hole bundle");
    };
    let record = bundle.hole_record().unwrap();
    assert_eq!(record.setup.hole, 99);
    assert_eq!(record.setup.seed.0, 5);
    assert_eq!(bundle.manifest.focus.stroke, 0);
    assert!(record.entries.is_empty());
}

fn lip_out_for_build(dir: &Scratch) -> PathBuf {
    let path = fixture("lip-out");
    if GOLF_SIM_COMMIT == "dev" {
        return path;
    }
    // Release builds stamp their commit into the record.
    let ReproBundle::Hole(mut bundle) = ReproBundle::read(&path).unwrap() else {
        panic!("expected a hole bundle");
    };
    let mut record = decode_envelope(&bundle.record).unwrap();
    let Some(Variant::Dict(versions)) = record.get_mut("versions") else {
        panic!("expected record versions");
    };
    *versions.get_mut("commit").unwrap() = GOLF_SIM_COMMIT.into();
    let body = variant::encode(&Variant::Dict(record)).unwrap();
    bundle.record.truncate(8);
    bundle
        .record
        .extend_from_slice(&u32::try_from(body.len()).unwrap().to_le_bytes());
    bundle.record.extend_from_slice(&body);
    bundle.manifest.engine.commit = GOLF_SIM_COMMIT.into();
    bundle.manifest.extra.get_mut("versions").unwrap()["commit"] = json!(GOLF_SIM_COMMIT);
    let path = dir.0.join("lip-out");
    bundle.write(&path).unwrap();
    path
}

fn check_number_sources(account: &str, trace: &Trace) {
    for line in account.lines() {
        for segment in line.split_inclusive(']') {
            let (text, source) = segment
                .split_once('[')
                .map_or((segment, None), |(text, source)| {
                    (text, source.strip_suffix(']'))
                });
            let numbers: Vec<_> = text
                .split([' ', '(', ')', ',', ';'])
                .filter(|word| word.trim_end_matches('.').parse::<f64>().is_ok())
                .collect();
            if numbers.is_empty() {
                assert!(
                    source.is_none(),
                    "source without a measurement in {segment}"
                );
                continue;
            }
            assert_eq!(
                numbers.len(),
                1,
                "one source is required per number in {segment}"
            );
            let source = source.unwrap_or_else(|| panic!("number without a source in {segment}"));
            assert!(
                trace
                    .phases()
                    .iter()
                    .flat_map(|entry| &entry.values)
                    .any(|value| {
                        let (kind, name) = match &value.source {
                            Source::Computed => ("computed", &value.name),
                            Source::Tunable { name } => ("tunable", name),
                            Source::Turf { name } => ("turf", name),
                            Source::Course { name } => ("course", name),
                            Source::SeedDraw { name } => ("seed draw", name),
                        };
                        source
                            .strip_prefix(kind)
                            .and_then(|suffix| suffix.strip_prefix(' '))
                            == Some(name)
                    }),
                "source {source} did not come from the trace"
            );
        }
    }
}

fn golf() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_golf"));
    let paths = [data().join("no-courses-here"), data().join("courses")];
    command.env("GOLF_COURSES", env::join_paths(paths).unwrap());
    command
}

fn success_text(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn failure_text(output: Output) -> String {
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty());
    String::from_utf8(output.stderr).unwrap()
}

fn data() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

fn course() -> PathBuf {
    data().join("courses/test-course")
}

fn fixture(name: &str) -> PathBuf {
    data().join("bundles").join(name)
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = env::temp_dir().join(format!(
            "golf-repro-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
