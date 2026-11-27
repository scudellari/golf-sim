use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};

use golf_sim::{
    base::{
        digest::sha256_hex,
        variant::{self, Variant},
    },
    records::hole::{Entry, HoleRecord, decode_envelope},
};
use golf_sim_testkit::items::hole::{masked_record_bytes, record_for_current_build};
use serde_json::{Value, json};

const REBLESS: &str = "Raise the protocol version and re-bless with \
    `cargo run --offline -p golf-sim-cli -- schema > crates/golf-sim-cli/schema.json`.";

#[test]
fn caddie_transcript_holes_out_and_saves_the_committed_record() {
    let (replies, bytes) = transcript("caddie", &[]);
    assert_eq!(replies[8]["ok"]["stroke"]["end"], "holed");
    assert_eq!(replies[9]["ok"]["prompt"], json!({"done": {}}));
    assert_eq!(replies[10]["ok"]["actions"], json!([]));
    assert_eq!(
        replies[4]["refused"],
        json!({"not_now": {"prompt": "play"}})
    );

    let reference = fs::read(data().join("cli/caddie.rec")).unwrap();
    assert_eq!(bytes, record_for_current_build(&reference));
    let record = HoleRecord::from_bytes(&bytes).unwrap();
    assert_eq!(
        record.versions.get("release").and_then(Variant::as_str),
        Some(env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(record.setup.hole, 1);
    assert_eq!(record.entries.len(), 3);
    for entry in record.entries {
        let Entry::Stroke(stroke) = entry else {
            panic!("expected a stroke")
        };
        assert_eq!(
            stroke.swing.get("mode").and_then(Variant::as_str),
            Some("exact")
        );
    }
    println!(
        "Caddie holed out in 3 strokes, 12 replies matched, and the restamped record matched bit for bit"
    );
}

#[test]
fn profile_transcript_has_the_same_replies_and_record_in_three_runs() {
    let args = ["--profile", "hcp15", "--seed", "7"];
    let (_, expected) = transcript("profile", &args);
    let expected_bytes = masked_record_bytes(&expected);
    for _ in 0..2 {
        let (_, actual) = transcript("profile", &args);
        assert_eq!(masked_record_bytes(&actual), expected_bytes);
    }
    let record = HoleRecord::from_bytes(&expected).unwrap();
    assert_eq!(record.setup.seed.0, 7);
    assert_eq!(record.entries.len(), 2);
    let preset = golf_sim_testkit::items::hole::items()
        .into_iter()
        .find(|item| item.id == "stroke.hcp15")
        .unwrap();
    let Variant::PackedByteArray(preset) = (preset.run)() else {
        panic!("expected a preset record");
    };
    let preset = decode_envelope(&preset).unwrap();
    let Some(Variant::Array(entries)) = preset.get("entries") else {
        panic!("expected preset record entries");
    };
    let swing = entries.items[0].as_dict().unwrap().get("swing").unwrap();
    for entry in record.entries {
        let Entry::Stroke(stroke) = entry else {
            panic!("expected a stroke")
        };
        assert_eq!(&Variant::Dict(stroke.swing), swing);
    }
    let input = fs::read_to_string(data().join("cli/profile.jsonl")).unwrap();
    let explicit = input.replace(
        "\"club\":\"driver\",",
        "\"club\":\"driver\",\"swing\":{\"profile\":\"hcp15\"},",
    );
    let dir = Scratch::new();
    assert!(play(&dir, explicit.as_bytes(), &args).status.success());
    let explicit_record = fs::read(dir.0.join("target/c27-profile.rec")).unwrap();
    assert_eq!(masked_record_bytes(&explicit_record), expected_bytes);
    println!("All three hcp15 runs matched 5 expected replies and wrote identical records");
}

#[test]
fn bad_lines_are_refused_and_the_session_can_still_record_and_save() {
    let dir = Scratch::new();
    let mut input = br#"{"id":"unknown","op":"teleport"}
{"id":2,"op":"preview","stroke":{}}
{"id":3,
{"id":4,"op":"act","action":{"stroke":{"club":"driver","aim_at":[414.0,74.0]}}}
{"id":5,"op":"save","path":"missing/record.rec"}
{"id":6,"op":"record"}
{"id":7,"op":"save","path":"target/saved.rec"}
"#
    .to_vec();
    input.extend_from_slice(&[0xff, b'\n']);
    input.extend_from_slice(br#"{"id":9,"op":"legal"}"#);
    let replies = parse_lines(&success(play(&dir, &input, &[])));
    assert_eq!(replies.len(), 9);
    assert_eq!(
        replies[0],
        json!({"id":"unknown","refused":{"unknown_op":{"op":"teleport"}}})
    );
    assert_eq!(replies[1]["id"], json!(2));
    assert!(replies[1]["refused"]["malformed"]["message"].is_string());
    assert_eq!(replies[2]["id"], Value::Null);
    assert!(replies[2]["refused"]["malformed"]["message"].is_string());
    assert_eq!(replies[3], json!({"id":4,"refused":{"missing_swing":{}}}));
    assert_eq!(
        replies[4]["refused"]["bad_path"]["path"],
        json!("missing/record.rec")
    );
    let bytes = fs::read(dir.0.join("target/saved.rec")).unwrap();
    assert_eq!(replies[5]["ok"]["bytes"], json!(bytes));
    assert!(HoleRecord::from_bytes(&bytes).unwrap().entries.is_empty());
    assert_eq!(replies[6], json!({"id":7,"ok":{"path":"target/saved.rec"}}));
    assert!(replies[7]["refused"]["malformed"]["message"].is_string());
    assert!(replies[8]["ok"]["actions"].is_array());
}

#[test]
fn replay_names_the_first_diverging_field_and_both_values() {
    let dir = Scratch::new();
    let path = dir.0.join("caddie.rec");
    let mut bytes = record_for_current_build(&fs::read(data().join("cli/caddie.rec")).unwrap());
    fs::write(&path, &bytes).unwrap();
    let replay = || golf().arg("replay").arg(&path).output().unwrap();

    let output = success(replay());
    assert_eq!(output, "All 3 entries replayed bit for bit.\n");
    print!("{output}");

    let mut body = decode_envelope(&bytes).unwrap();
    let Some(Variant::Array(entries)) = body.get_mut("entries") else {
        panic!("expected entries");
    };
    let Variant::Dict(entry) = &mut entries.items[1] else {
        panic!("expected an approach stroke");
    };
    let Some(Variant::Vector3(rest)) = entry.get_mut("rest") else {
        panic!("expected a stored rest position");
    };
    let computed = rest.values[0];
    let saved = f64::from_bits(computed.to_bits() ^ 1);
    rest.values[0] = saved;
    let changed = variant::encode(&Variant::Dict(body)).unwrap();
    assert_eq!(bytes.len() - 12, changed.len());
    assert_eq!(
        bytes[12..]
            .iter()
            .zip(&changed)
            .map(|(a, b)| (a ^ b).count_ones())
            .sum::<u32>(),
        1
    );
    bytes[12..].copy_from_slice(&changed);
    fs::write(&path, bytes).unwrap();

    let output = replay();
    assert_eq!(output.status.code(), Some(1));
    let report = String::from_utf8(output.stderr).unwrap();
    assert!(
        report.contains(&format!(
            "entry 1 diverges at rest.x: stored {saved:?}, computed {computed:?}"
        )),
        "{report}"
    );
    print!("{report}");
}

#[test]
fn schema_matches_the_snapshot_and_protocol_one_is_frozen() {
    let actual = success(golf().arg("schema").output().unwrap());
    let committed = include_str!("../schema.json");
    assert!(actual == committed, "{REBLESS}");
    let schema: Value = serde_json::from_str(committed).unwrap();
    if schema["protocol"] == json!(1) {
        assert_eq!(
            sha256_hex(actual.as_bytes()),
            "a88b35f37c140b4b85b32d9d1087da1c4eb0f65f81fb7b0ff1106bd68b5e6439",
            "{REBLESS}"
        );
    }
    println!(
        "Schema matches the committed snapshot for protocol {}",
        schema["protocol"]
    );
}

fn transcript(name: &str, args: &[&str]) -> (Vec<Value>, Vec<u8>) {
    let dir = Scratch::new();
    let path = data().join("cli");
    let script = fs::read(path.join(format!("{name}.jsonl"))).unwrap();
    let expected = fs::read_to_string(path.join(format!("{name}.out.jsonl"))).unwrap();
    let actual = success(play(&dir, &script, args));
    assert_eq!(actual.lines().count(), expected.lines().count(), "{name}");
    for (index, (received, expected)) in actual.lines().zip(expected.lines()).enumerate() {
        assert_eq!(received, expected, "{name} reply {}", index + 1);
    }
    let bytes = fs::read(dir.0.join(format!("target/c27-{name}.rec"))).unwrap();
    (parse_lines(&actual), bytes)
}

fn parse_lines(text: &str) -> Vec<Value> {
    text.lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn play(dir: &Scratch, input: &[u8], args: &[&str]) -> Output {
    let mut child = golf()
        .args(["play", "--course"])
        .arg(data().join("courses/test-course"))
        .args(["--hole", "1"])
        .args(args)
        .current_dir(&dir.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn golf() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_golf"));
    command.env("GOLF_COURSES", data().join("courses"));
    command
}

fn data() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = env::temp_dir().join(format!(
            "golf-play-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::create_dir(path.join("target")).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
