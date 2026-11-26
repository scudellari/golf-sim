use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};

use golf_sim_base::{digest::digest, variant::Variant};

pub struct Item {
    pub id: &'static str,
    pub run: fn() -> Variant,
}

impl Item {
    pub const fn new(id: &'static str, run: fn() -> Variant) -> Self {
        Self { id, run }
    }
}

type Items = fn() -> Vec<Item>;
type Digests = BTreeMap<String, String>;

fn empty_items() -> Vec<Item> {
    Vec::new()
}

// A layer's items() shadows the fallback as soon as its stub is filled.
macro_rules! layer_items {
    ($layer:ident) => {{
        #[allow(unused_imports)]
        use self::empty_items as items;
        {
            #[allow(unused_imports)]
            use crate::items::$layer::*;
            items as Items
        }
    }};
}

pub const LAYERS: &[(&str, Items)] = &[
    ("base", crate::items::base::items),
    ("generator", layer_items!(generator)),
    ("hands", layer_items!(hands)),
    ("hole", layer_items!(hole)),
    ("physics", layer_items!(physics)),
    ("round", layer_items!(round)),
];

pub fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("testdata")
        .join("determinism")
}

pub fn verify(dir: &Path, layer: &str, items: &[Item]) -> Result<(), String> {
    let path = dir.join(format!("{layer}.json"));
    let expected: Digests = match fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?
        }
        Err(e) if e.kind() == ErrorKind::NotFound => BTreeMap::new(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let actual = digests(layer, items)?;
    let mut differences = String::new();
    for id in expected
        .keys()
        .chain(actual.keys())
        .collect::<BTreeSet<_>>()
    {
        let expected = expected.get(id);
        let actual = actual.get(id);
        if expected != actual {
            writeln!(
                differences,
                "{layer}/{id}: expected {}, actual {}",
                expected.map_or("<missing>", String::as_str),
                actual.map_or("<missing>", String::as_str),
            )
            .unwrap();
        }
    }
    if differences.is_empty() {
        Ok(())
    } else {
        Err(differences)
    }
}

pub fn bless(dir: &Path, layer: &str, items: &[Item]) -> Result<(), String> {
    let path = dir.join(format!("{layer}.json"));
    if items.is_empty() {
        return match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        };
    }

    let actual = digests(layer, items)?;
    let mut json = serde_json::to_string_pretty(&actual).map_err(|e| format!("{layer}: {e}"))?;
    json.push('\n');
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;

    // Commit a complete fixture even if writing the replacement fails partway through.
    let staged = dir.join(format!(".{layer}.{}.tmp", std::process::id()));
    let mut file =
        fs::File::create_new(&staged).map_err(|e| format!("{}: {e}", staged.display()))?;
    let written = file.write_all(json.as_bytes());
    drop(file);
    let result = written.and_then(|()| fs::rename(&staged, &path));
    if result.is_err() {
        let _ = fs::remove_file(&staged);
    }
    result.map_err(|e| format!("{}: {e}", path.display()))
}

fn digests(layer: &str, items: &[Item]) -> Result<Digests, String> {
    let mut digests = BTreeMap::new();
    for item in items {
        let hash = digest(&(item.run)()).map_err(|e| format!("{layer}/{}: {e}", item.id))?;
        if digests.insert(item.id.to_owned(), hash).is_some() {
            return Err(format!("{layer}/{}: duplicate item id", item.id));
        }
    }
    Ok(digests)
}
