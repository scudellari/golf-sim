use std::{
    path::PathBuf,
    process::{Command, ExitCode},
};

use serde_json::Value;

fn test_course() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/courses/test-course")
}

#[test]
fn checks_the_course_and_plays_a_drive_through_run() {
    let path = test_course();
    let course = path.to_str().unwrap();
    let drive = [
        "stroke", "--course", course, "--hole", "1", "--club", "driver",
    ];
    assert_eq!(
        golf_sim_cli::run(["golf", "course", "check", course]),
        ExitCode::SUCCESS
    );
    assert_eq!(
        golf_sim_cli::run(["golf"].into_iter().chain(drive)),
        ExitCode::SUCCESS
    );

    let output = Command::new(env!("CARGO_BIN_EXE_golf"))
        .args(drive)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(result["carry_m"].as_f64().unwrap() > 150.0);
    assert_eq!(result["end_reason"], "rest");
    assert_eq!(result["outcome"]["end"], "at_rest");
    assert_eq!(result["rest"], result["outcome"]["rest"]);
    assert_eq!(
        result["events"].as_array().unwrap().first().unwrap()["kind"],
        "impact"
    );

    let output = Command::new(env!("CARGO_BIN_EXE_golf"))
        .args(["course", "check", course])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "test-course 82b40c2b1d877f0d5a28c48efdd2179e79e4be3ef664b69ec0f7b2e3569072eb 3 holes\n"
    );
}

#[test]
fn course_check_reports_a_missing_bundle_and_its_path() {
    let missing = test_course().join("manifest.json");
    let output = Command::new(env!("CARGO_BIN_EXE_golf"))
        .args(["course", "check"])
        .arg(&missing)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains(missing.to_str().unwrap()));
}
