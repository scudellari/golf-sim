use std::{
    env, fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

use golf_sim::base::digest::sha256_hex;
use serde_json::Value;

fn data() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

fn golf() -> Command {
    Command::new(env!("CARGO_BIN_EXE_golf"))
}

fn sweep(dir: &Scratch) -> Command {
    let mut command = golf();
    // Sweep's batch subprocess writes repros relative to its working directory.
    command
        .current_dir(&dir.0)
        .arg("sweep")
        .arg("--course")
        .arg(data().join("courses/test-course"));
    command
}

fn success(output: Output) -> Vec<u8> {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

#[test]
fn sweep_bytes_are_independent_of_worker_count() {
    let dir = Scratch::new();
    for format in [&[][..], &["--json"][..]] {
        let run = |jobs| {
            success(
                sweep(&dir)
                    .args([
                        "--holes",
                        "2,1",
                        "--days",
                        "constant",
                        "--profiles",
                        "scratch,hcp15",
                        "--seeds",
                        "1-4",
                        "--jobs",
                        jobs,
                    ])
                    .args(format)
                    .output()
                    .unwrap(),
            )
        };
        let serial = run("1");
        assert_eq!(serial, run("8"));
        let kind = if format.is_empty() { "text" } else { "JSON" };
        println!(
            "{kind}: identical with 1 and 8 workers, {} bytes, SHA-256 {}",
            serial.len(),
            sha256_hex(&serial)
        );
        if format.is_empty() {
            let report = String::from_utf8(serial).unwrap();
            assert!(report.contains("Mean StdDev Fairways GIR Putts Penalties"));
            assert!(report.contains("Target       Distance(m) Bearing(deg)"));
            assert!(report.contains("fairway    landing_area"));
        } else {
            let report: Value = serde_json::from_slice(&serial).unwrap();
            let rows = report["holes"].as_array().unwrap();
            let order: Vec<_> = rows
                .iter()
                .map(|row| {
                    (
                        row["hole"].as_i64().unwrap(),
                        row["day"].as_str().unwrap(),
                        row["profile"].as_str().unwrap(),
                    )
                })
                .collect();
            assert_eq!(
                order,
                [
                    (1, "constant", "hcp15"),
                    (1, "constant", "scratch"),
                    (2, "constant", "hcp15"),
                    (2, "constant", "scratch"),
                ]
            );
            assert!(rows.iter().all(|row| row["plays"] == 4));
            let finishes = rows[0]["finishes"].as_array().unwrap();
            assert!(finishes.windows(2).all(|pair| {
                let key = |finish: &Value| {
                    (
                        finish["seed"].as_i64().unwrap(),
                        finish["stroke"].as_u64().unwrap(),
                    )
                };
                key(&pair[0]) < key(&pair[1])
            }));
            let first = &finishes[0];
            assert_eq!(first["lie"], "normal");
            assert_eq!(first["surface"], "fairway");
            assert_eq!(first["target"], "landing_area");
            assert!((first["distance_m"].as_f64().unwrap() - 38.59).abs() < 0.01);
            assert!((first["bearing_deg"].as_f64().unwrap() - 278.30).abs() < 0.01);
            assert!(finishes.iter().any(|finish| finish["target"] == "flag"));
        }
    }
}

#[test]
fn sweep_reports_score_distribution_and_totals() {
    let dir = Scratch::new();
    let bytes = success(
        sweep(&dir)
            .args([
                "--holes",
                "1",
                "--profiles",
                "hcp15",
                "--seeds",
                "1-2",
                "--json",
            ])
            .output()
            .unwrap(),
    );
    let report: Value = serde_json::from_slice(&bytes).unwrap();
    let row = &report["holes"][0];
    assert_eq!(row["score_counts"], serde_json::json!({"3": 1, "4": 1}));
    assert_eq!(row["mean"], 3.5);
    assert_eq!(row["std_dev"], 0.5);
    assert_eq!(row["fairways_hit"], 2);
    assert_eq!(row["greens_in_regulation"], 2);
    assert_eq!(row["putts"], 3);
    assert_eq!(row["penalties"], 0);
    assert_eq!(row["holed"], 2);
    assert_eq!(row["finishes"].as_array().unwrap().len(), 7);
}

#[test]
fn generated_days_are_refused_until_the_generator_lands() {
    let dir = Scratch::new();
    let output = sweep(&dir)
        .args(["--days", "1-20", "--seeds", "1-1"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(
        error.contains("Day seed ranges require the day generator"),
        "{error}"
    );
}

#[test]
fn refused_sweep_plays_save_repros_in_scratch() {
    let dir = Scratch::new();
    let output = sweep(&dir)
        .args(["--holes", "99", "--seeds", "5-5"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("hole 99"), "{error}");
    assert!(
        dir.0
            .join("repros/batch-hole-99-seed-5/record.bin")
            .is_file()
    );
}

#[test]
fn all_test_course_scenarios_pass() {
    let dir = Scratch::new();
    let testdata = data();
    let bytes = success(
        golf()
            .args(["scenario", "run", "--repros"])
            .arg(dir.0.join("repros"))
            .arg(testdata.join("scenarios"))
            .output()
            .unwrap(),
    );
    let report = String::from_utf8(bytes).unwrap();
    for (hole, score) in [(1, 4), (2, 2), (3, 4)] {
        assert!(report.contains(&format!(
            "hole-{hole}.toml: passed ({score} strokes, score {score})"
        )));
    }
}

#[test]
fn scenario_names_the_first_failed_expectation_and_bless_repairs_it() {
    let scratch = Scratch::new();
    let file = scratch.0.join("edited.toml");
    let testdata = data();
    let mut document: toml::Table =
        toml::from_str(&fs::read_to_string(testdata.join("scenarios/hole-2.toml")).unwrap())
            .unwrap();
    let course = testdata.join("courses/test-course").canonicalize().unwrap();
    document.insert("course".into(), course.to_str().unwrap().into());
    let expected = document
        .get_mut("expected")
        .unwrap()
        .as_table_mut()
        .unwrap();
    expected.insert("score".into(), 99.into());
    let strokes = expected.get_mut("strokes").unwrap().as_array_mut().unwrap();
    strokes[0]
        .as_table_mut()
        .unwrap()
        .insert("end".into(), "holed".into());
    fs::write(&file, toml::to_string(&document).unwrap()).unwrap();
    let run = || {
        golf()
            .args(["scenario", "run", "--repros"])
            .arg(scratch.0.join("repros"))
            .arg(&file)
            .output()
            .unwrap()
    };
    let output = run();
    assert_eq!(output.status.code(), Some(1));
    let error = String::from_utf8(output.stderr).unwrap();
    let failure = "expected.strokes[0].end: expected \"holed\", got \"at_rest\"";
    assert!(
        error.contains(&format!("edited.toml: {failure}")),
        "{error}"
    );
    assert!(!error.contains("expected.score"), "{error}");
    println!("{failure} (exit 1)");

    success(
        golf()
            .args(["scenario", "bless"])
            .arg(&file)
            .output()
            .unwrap(),
    );
    let mut blessed: toml::Table = toml::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(
        blessed.remove("expected").unwrap()["score"].as_integer(),
        Some(2)
    );
    document.remove("expected");
    assert_eq!(blessed, document);
    success(run());
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = env::temp_dir().join(format!(
            "golf-sweep-test-{}-{}",
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
