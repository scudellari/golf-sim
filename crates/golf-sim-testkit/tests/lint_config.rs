//! The determinism lints in clippy.toml catch what they are meant to catch.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

fn crate_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn workspace_dir() -> PathBuf {
    crate_dir().join("../..")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn clippy_flags_every_banned_line_of_the_fixture_and_no_other() {
    let fixture = crate_dir().join("lint-fixture");
    let source = read(&fixture.join("src/lib.rs"));
    let banned: BTreeSet<usize> = source
        .lines()
        .enumerate()
        .filter(|(_, line)| line.ends_with("// banned"))
        .map(|(i, _)| i + 1)
        .collect();

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let output = Command::new(cargo)
        .args([
            "clippy",
            "--offline",
            "--quiet",
            "--message-format=short",
            "--manifest-path",
        ])
        .arg(fixture.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(Path::new(env!("CARGO_TARGET_TMPDIR")).join("lint-fixture"))
        .args(["--", "-D", "warnings"])
        .env_remove("CLIPPY_CONF_DIR")
        .output()
        .expect("cargo clippy runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "clippy passed the fixture:\n{stderr}"
    );

    let flagged: BTreeSet<usize> = stderr
        .lines()
        .filter(|line| line.contains("disallowed"))
        .filter_map(|line| {
            line.strip_prefix("src/lib.rs:")
                .or_else(|| line.strip_prefix("src\\lib.rs:"))
        })
        .filter_map(|rest| rest.split(':').next()?.parse().ok())
        .collect();
    assert_eq!(
        flagged, banned,
        "lines flagged by clippy against lines marked banned:\n{stderr}"
    );
}

/// Clippy reads the nearest clippy.toml, so the crates that may start threads keep their own copy.
#[test]
fn crate_copies_of_clippy_toml_match_the_workspace_one_without_the_thread_entries() {
    let rules = |text: &str| -> Vec<String> {
        text.lines()
            .filter(|line| !line.starts_with('#') && !line.contains("std::thread"))
            .map(str::to_owned)
            .collect()
    };
    let workspace = rules(&read(&workspace_dir().join("clippy.toml")));
    assert!(workspace.iter().any(|line| line.contains("f64::sin")));
    for name in ["golf-sim-cli", "golf-sim-godot"] {
        let copy = read(
            &workspace_dir()
                .join("crates")
                .join(name)
                .join("clippy.toml"),
        );
        assert!(!copy.contains("std::thread"), "{name} bans threads");
        let copy = rules(&copy);
        let only_copy: Vec<_> = copy.iter().filter(|l| !workspace.contains(l)).collect();
        let only_workspace: Vec<_> = workspace.iter().filter(|l| !copy.contains(l)).collect();
        assert!(
            copy == workspace,
            "crates/{name}/clippy.toml differs from clippy.toml\nonly in the copy: {only_copy:#?}\nonly in clippy.toml: {only_workspace:#?}"
        );
    }
}
