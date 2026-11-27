use std::{
    collections::BTreeMap,
    env,
    ffi::OsStr,
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

use golf_sim::{
    base::{PromptKind, Vec2, corpus, digest::sha256_hex, variant::Variant},
    course::course::Course,
    hole::Hole,
    records::hole::Entry,
};
use serde_json::Value;

fn data() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

fn batch(args: impl IntoIterator<Item = impl AsRef<OsStr>>) -> Vec<u8> {
    let scratch = Scratch::new();
    let output = Command::new(env!("CARGO_BIN_EXE_golf"))
        .arg("batch")
        .arg("--course")
        .arg(data().join("courses/test-course"))
        .arg("--repros")
        .arg(scratch.0.join("repros"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn rows(bytes: &[u8]) -> impl Iterator<Item = Value> {
    std::str::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
}

#[test]
fn worker_count_preserves_json_bytes_for_exact_and_profile_swings() {
    for swing in [&["--exact"][..], &["--profile", "hcp15"][..]] {
        let run = |jobs| {
            let mut args = vec!["--holes", "1", "--seeds", "1-200", "--jobs", jobs];
            args.extend(swing);
            batch(&args)
        };
        let serial = run("1");
        let parallel = run("8");
        assert_eq!(serial, parallel, "{swing:?}");
        let results: Vec<_> = rows(&serial).collect();
        assert_eq!(results.len(), 200);
        for (index, row) in results.into_iter().enumerate() {
            assert_eq!(row["seed"], index + 1);
            assert_eq!(row["hole"], 1);
            assert!(row["holed"].is_boolean(), "{row}");
            assert!(
                row["strokes"]
                    .as_u64()
                    .is_some_and(|n| (1..=20).contains(&n)),
                "{row}"
            );
        }
        println!(
            "{}: 200 identical JSON lines, {} bytes, SHA-256 {}",
            swing.last().unwrap(),
            serial.len(),
            sha256_hex(&serial)
        );
    }
}

#[test]
fn exact_golfer_matches_most_reference_scores() {
    let fixture = corpus::read(&data().join("records/holes/fixture.bin")).unwrap();
    let Some(Variant::Array(cases)) = fixture.as_dict().unwrap().get("cases") else {
        panic!("expected hole cases");
    };
    let expected: BTreeMap<_, _> = cases
        .items
        .iter()
        .map(|case| case.as_dict().unwrap())
        .filter_map(|case| {
            let id = case.get("id").unwrap().as_str().unwrap();
            id.starts_with("play/test-course/").then(|| {
                let output = case.get("output").unwrap().as_dict().unwrap();
                let Some(Variant::Int(strokes)) = output.get("strokes") else {
                    panic!("{id}: expected a stroke count");
                };
                (id.to_string(), strokes.value())
            })
        })
        .collect();
    assert_eq!(expected.len(), 30);

    let mut actual = BTreeMap::new();
    for mode in ["caddie", "tour"] {
        let bytes = batch(["--exact", "--seeds", "1-5", "--mode", mode, "--jobs", "4"]);
        for row in rows(&bytes) {
            let id = format!("play/test-course/{}/{}/{mode}", row["hole"], row["seed"]);
            let strokes = row["strokes"]
                .as_i64()
                .unwrap_or_else(|| panic!("{id}: {row}"));
            assert!(actual.insert(id, strokes).is_none());
        }
    }
    assert!(expected.keys().eq(actual.keys()));
    let mismatches: Vec<_> = expected
        .iter()
        .filter_map(|(id, reference)| {
            let played = actual[id];
            (played != *reference).then(|| format!("{id}: batch {played}, reference {reference}"))
        })
        .collect();
    let matches = expected.len() - mismatches.len();
    let report = format!(
        "{matches}/30 exact scores matched\n{}",
        mismatches.join("\n")
    );
    println!("{report}");
    assert!(matches >= 27, "{report}");
}

#[test]
fn batch_saves_replayable_records_and_continues_past_refused_plays() {
    let records = Scratch::new();
    let mut args: Vec<&OsStr> = [
        "--holes",
        "99,1",
        "--seeds",
        "-1-0",
        "--jobs",
        "2",
        "--exact",
        "--records",
    ]
    .map(OsStr::new)
    .into();
    args.push(records.0.as_os_str());
    let bytes = batch(args);
    let results: Vec<_> = rows(&bytes).collect();
    assert_eq!(results.len(), 4);
    assert_eq!(fs::read_dir(&records.0).unwrap().count(), 2);
    let course = Course::open(&data().join("courses/test-course")).unwrap();
    let (pairs, remainder) = results.as_chunks::<2>();
    assert!(remainder.is_empty());
    for (seed, [played, failed]) in [-1, 0].into_iter().zip(pairs) {
        assert_eq!(played["seed"], seed);
        assert_eq!(played["hole"], 1);
        assert_eq!(failed["seed"], seed);
        assert_eq!(failed["hole"], 99);
        assert!(failed["error"].is_string());
        let record = fs::read(records.0.join(format!("hole-1-seed-{seed}.rec"))).unwrap();
        let hole = Hole::resume(course.clone(), &record).unwrap();
        assert_eq!(hole.record().setup.seed.0, seed);
        assert_eq!(
            hole.rules().strokes(),
            played["strokes"].as_u64().unwrap() as usize
        );
        assert_eq!(hole.prompt().kind == PromptKind::Done, played["holed"]);
        for entry in &hole.record().entries {
            if let Entry::Stroke(stroke) = entry {
                assert_eq!(stroke.facts.execution.timing_fault, 0.0);
                assert_eq!(stroke.facts.execution.strike_m, Some(Vec2::ZERO));
            }
        }
    }
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = env::temp_dir().join(format!(
            "golf-batch-{}-{}",
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
