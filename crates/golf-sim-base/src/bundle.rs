//! The folder container for a repro record and its attachments.

use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, Write},
    path::Path,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Error, GOLF_SIM_COMMIT};

const FORMAT: i64 = 1;
const MANIFEST: &str = "bundle.json";
pub const RECORD: &str = "record.bin";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BundleManifest {
    pub format: i64,
    pub engine: BundleEngine,
    /// Unix time in seconds.
    pub time: u64,
    pub note: String,
    pub course: BundleCourse,
    pub focus: BundleFocus,
    pub files: Vec<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BundleEngine {
    pub release: String,
    pub commit: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl BundleEngine {
    pub fn current() -> Self {
        Self {
            release: env!("CARGO_PKG_VERSION").into(),
            commit: GOLF_SIM_COMMIT.into(),
            extra: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BundleCourse {
    pub id: String,
    pub revision: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BundleFocus {
    pub hole: Option<i64>,
    /// The one-based stroke number within its hole or practice world.
    pub stroke: usize,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl BundleManifest {
    pub fn new(course: BundleCourse, focus: BundleFocus, time: u64, note: String) -> Self {
        Self {
            format: FORMAT,
            engine: BundleEngine::current(),
            time,
            note,
            course,
            focus,
            files: vec![RECORD.into()],
            extra: BTreeMap::new(),
        }
    }

    fn validate(&self) -> Result<(), Error> {
        if self.format != FORMAT {
            return Err(Error::UnknownVersion {
                what: "repro bundle",
                version: self.format,
            });
        }
        if !self.files.iter().any(|file| file == RECORD) {
            return Err(Error::MissingKey {
                path: format!("{MANIFEST}.files[\"{RECORD}\"]"),
            });
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct Bundle {
    pub manifest: BundleManifest,
    pub record: Vec<u8>,
}

impl Bundle {
    pub fn read(dir: &Path) -> Result<Self, Error> {
        let path = dir.join(MANIFEST);
        let manifest: BundleManifest =
            serde_json::from_slice(&read(&path)?).map_err(|error| malformed(&path, error))?;
        manifest.validate()?;
        let record = read(&dir.join(RECORD))?;
        Ok(Self { manifest, record })
    }

    /// Writes a manifest and record to a directory without replacing any files.
    pub fn write(&self, dir: &Path) -> Result<(), Error> {
        self.manifest.validate()?;
        let manifest_path = dir.join(MANIFEST);
        let mut manifest = serde_json::to_vec_pretty(&self.manifest)
            .map_err(|error| malformed(&manifest_path, error))?;
        manifest.push(b'\n');
        fs::create_dir_all(dir).map_err(|error| io_error(dir, error))?;
        let record_path = dir.join(RECORD);
        write_new(&record_path, &self.record)?;
        // Write the manifest last so it only points to a complete record.
        if let Err(error) = write_new(&manifest_path, &manifest) {
            let _ = fs::remove_file(&record_path);
            return Err(error);
        }
        Ok(())
    }
}

fn read(path: &Path) -> Result<Vec<u8>, Error> {
    fs::read(path).map_err(|error| io_error(path, error))
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let mut file = File::create_new(path).map_err(|error| io_error(path, error))?;
    let result = file.write_all(bytes);
    drop(file);
    if let Err(error) = result {
        let _ = fs::remove_file(path);
        return Err(io_error(path, error));
    }
    Ok(())
}

fn io_error(path: &Path, error: io::Error) -> Error {
    Error::Io {
        file: path.display().to_string(),
        message: error.to_string(),
    }
}

fn malformed(path: &Path, error: serde_json::Error) -> Error {
    Error::Malformed {
        path: path.display().to_string(),
        message: error.to_string(),
    }
}
