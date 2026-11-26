use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use golf_sim_base::digest::digest;
use golf_sim_testkit::determinism::{Item, LAYERS, bless, fixture_dir, verify};

#[test]
fn every_layer_matches_its_digests() {
    let dir = fixture_dir();
    let failures: Vec<_> = LAYERS
        .iter()
        .filter_map(|(layer, items)| verify(&dir, layer, &items()).err())
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn changed_item_fails_until_blessed() {
    let temp = TempDir::new();
    let dir = temp.0.join("determinism");
    let path = dir.join("sample.json");
    let before = [
        Item::new("retired", || 3_i64.into()),
        Item::new("changed", || 1_i64.into()),
    ];
    let after = [
        Item::new("introduced", || 4_i64.into()),
        Item::new("changed", || 2_i64.into()),
    ];
    let old = digest(&1_i64.into()).unwrap();
    let new = digest(&2_i64.into()).unwrap();
    let retired = digest(&3_i64.into()).unwrap();
    let introduced = digest(&4_i64.into()).unwrap();

    verify(&dir, "sample", &[]).unwrap();
    let missing_file = verify(&dir, "sample", &before).unwrap_err();
    assert!(missing_file.contains(&format!("sample/changed: expected <missing>, actual {old}")));

    bless(&dir, "sample", &before).unwrap();
    verify(&dir, "sample", &before).unwrap();
    let failure = verify(&dir, "sample", &after).unwrap_err();
    assert_eq!(
        failure,
        format!(
            "sample/changed: expected {old}, actual {new}\n\
             sample/introduced: expected <missing>, actual {introduced}\n\
             sample/retired: expected {retired}, actual <missing>\n"
        )
    );
    print!("{failure}");

    bless(&dir, "sample", &after).unwrap();
    assert_eq!(
        fs::read(&path).unwrap(),
        format!("{{\n  \"changed\": \"{new}\",\n  \"introduced\": \"{introduced}\"\n}}\n")
            .as_bytes()
    );
    verify(&dir, "sample", &after).unwrap();
    println!("verification passed after bless");

    bless(&dir, "sample", &[]).unwrap();
    verify(&dir, "sample", &[]).unwrap();
    assert!(!path.exists());
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "golf-sim-determinism-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
