use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{Quantities, Quantity, Submodel, TargetFile, target::named_enum};

const LEDGER: &str = "targets/LICENCES.toml";

named_enum!(Verdict {
    Commit => "commit",
    Cite => "cite",
    Fetch => "fetch",
    Local => "local",
    Excluded => "excluded",
});

impl Verdict {
    fn optional(self) -> bool {
        matches!(self, Self::Fetch | Self::Local)
    }
}

#[derive(Debug, Deserialize)]
pub struct Ledger {
    pub format: u32,
    #[serde(default)]
    pub datasets: BTreeMap<String, Dataset>,
}

#[derive(Debug, Deserialize)]
pub struct Dataset {
    pub verdict: Verdict,
    #[serde(default)]
    pub files: Vec<PathBuf>,
    #[serde(default)]
    pub local_files: Vec<PathBuf>,
    pub fetch: Option<Fetch>,
}

#[derive(Debug, Deserialize)]
pub struct Fetch {
    pub url: String,
    pub step: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SkippedDataset {
    pub dataset: String,
    pub verdict: Verdict,
    pub missing_files: Vec<String>,
}

#[derive(Debug)]
pub struct LoadError {
    pub path: PathBuf,
    pub message: String,
}

impl LoadError {
    fn new(path: &Path, error: impl fmt::Display) -> Self {
        Self {
            path: path.to_owned(),
            message: error.to_string(),
        }
    }
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}

impl std::error::Error for LoadError {}

#[derive(Debug)]
pub struct Corpus {
    ledger: Ledger,
    files: Vec<TargetFile>,
    skipped: Vec<SkippedDataset>,
    pub(crate) layers: Layers,
}

impl Corpus {
    /// Load a validation directory containing `targets/LICENCES.toml`, with the
    /// layer tolerance table alongside the validation directory.
    pub fn load(validation_dir: impl AsRef<Path>) -> Result<Self, LoadError> {
        let root = validation_dir.as_ref();
        let ledger_path = root.join(LEDGER);
        let ledger: Ledger = read_toml(&ledger_path)?;
        check_format(&ledger_path, ledger.format)?;
        let mut present = BTreeSet::new();
        list_files(root, root, &mut present)?;

        let mut registered = BTreeMap::<PathBuf, BTreeSet<&str>>::new();
        let mut skipped = Vec::new();
        let mut needs_rows = BTreeSet::new();
        for (name, dataset) in &ledger.datasets {
            let mut missing_files = Vec::new();
            let mut has_present_files = false;
            for (directory, files) in [
                ("targets", &dataset.files),
                ("local", &dataset.local_files),
            ] {
                for file in files {
                    if file.as_os_str().is_empty()
                        || !file
                            .components()
                            .all(|part| matches!(part, Component::Normal(_)))
                    {
                        return Err(LoadError::new(
                            &ledger_path,
                            format!("dataset {name:?} has invalid file path {}", file.display()),
                        ));
                    }
                    let path = Path::new(directory).join(file);
                    registered.entry(path.clone()).or_default().insert(name);
                    if present.contains(&path) {
                        has_present_files = true;
                    } else if dataset.verdict != Verdict::Excluded {
                        if !dataset.verdict.optional() {
                            return Err(LoadError::new(
                                &root.join(path),
                                format!("file listed by dataset {name:?} is missing"),
                            ));
                        }
                        missing_files.push(
                            path.iter()
                                .map(|part| part.to_string_lossy())
                                .collect::<Vec<_>>()
                                .join("/"),
                        );
                    }
                }
            }
            if has_present_files && dataset.verdict != Verdict::Excluded {
                needs_rows.insert(name.as_str());
            }
            if dataset.verdict.optional() && (!has_present_files || !missing_files.is_empty()) {
                missing_files.sort();
                missing_files.dedup();
                skipped.push(SkippedDataset {
                    dataset: name.clone(),
                    verdict: dataset.verdict,
                    missing_files,
                });
            }
        }

        let mut files = Vec::new();
        let mut ids = BTreeSet::new();
        for path in present {
            if path == Path::new(LEDGER) {
                continue;
            }
            let full_path = root.join(&path);
            let names = registered
                .get(&path)
                .ok_or_else(|| LoadError::new(&full_path, "file has no licence ledger entry"))?;
            let active: BTreeSet<_> = names
                .iter()
                .copied()
                .filter(|name| ledger.datasets[*name].verdict != Verdict::Excluded)
                .collect();
            if active.is_empty() || path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let file = read_targets(&full_path, &ledger, &active)?;
            for target in &file.targets {
                if !ids.insert(target.id.clone()) {
                    return Err(LoadError::new(
                        &full_path,
                        format!("duplicate target id {:?}", target.id),
                    ));
                }
            }
            if !file.targets.is_empty() {
                for name in active {
                    needs_rows.remove(name);
                }
            }
            files.push(file);
        }
        if !needs_rows.is_empty() {
            return Err(LoadError::new(
                &ledger_path,
                format!("datasets with files give no scorecard row: {needs_rows:?}"),
            ));
        }
        let tolerance_path = root.with_file_name("tolerances.toml");
        let layers: Layers = read_toml(&tolerance_path)?;
        layers.validate(&tolerance_path, &files)?;
        Ok(Self {
            ledger,
            files,
            skipped,
            layers,
        })
    }

    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }

    pub fn files(&self) -> &[TargetFile] {
        &self.files
    }

    pub fn skipped_datasets(&self) -> &[SkippedDataset] {
        &self.skipped
    }
}

fn list_files(
    root: &Path,
    directory: &Path,
    files: &mut BTreeSet<PathBuf>,
) -> Result<(), LoadError> {
    for entry in fs::read_dir(directory).map_err(|error| LoadError::new(directory, error))? {
        let entry = entry.map_err(|error| LoadError::new(directory, error))?;
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|error| LoadError::new(&path, error))?;
        if kind.is_dir() {
            list_files(root, &path, files)?;
        } else {
            files.insert(
                path.strip_prefix(root)
                    .expect("listing the validation tree")
                    .into(),
            );
        }
    }
    Ok(())
}

fn read_toml<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, LoadError> {
    let data = fs::read_to_string(path).map_err(|error| LoadError::new(path, error))?;
    toml::from_str(&data).map_err(|error| LoadError::new(path, error))
}

fn check_format(path: &Path, format: u32) -> Result<(), LoadError> {
    if format != 1 {
        return Err(LoadError::new(path, format!("unknown format {format}")));
    }
    Ok(())
}

fn read_targets(
    path: &Path,
    ledger: &Ledger,
    active: &BTreeSet<&str>,
) -> Result<TargetFile, LoadError> {
    let data = fs::read(path).map_err(|error| LoadError::new(path, error))?;
    let file: TargetFile =
        serde_json::from_slice(&data).map_err(|error| LoadError::new(path, error))?;
    check_format(path, file.format)?;
    let declared: BTreeSet<_> = file.datasets.iter().map(String::as_str).collect();
    if declared != *active {
        return Err(LoadError::new(
            path,
            format!("file declares datasets {declared:?}, ledger lists {active:?}"),
        ));
    }
    if active
        .iter()
        .any(|name| ledger.datasets[*name].verdict != file.verdict)
    {
        return Err(LoadError::new(
            path,
            format!(
                "file verdict {} differs from the ledger for {active:?}",
                file.verdict
            ),
        ));
    }
    Ok(file)
}

#[derive(Debug, Deserialize)]
pub(crate) struct Layers {
    physics: Layer,
    stroke: Layer,
}

#[derive(Debug, Deserialize)]
struct Layer {
    #[serde(flatten)]
    default: Bounds,
    #[serde(default)]
    fields: BTreeMap<String, Bounds>,
}

#[derive(Debug, Deserialize)]
struct Bounds {
    #[serde(default)]
    abs: f64,
    #[serde(default)]
    rel: f64,
    #[serde(default)]
    f32_steps: f64,
    #[serde(default)]
    scale_from: Vec<String>,
}

impl Layer {
    fn bounds(&self, name: &str, si_name: &str) -> &Bounds {
        self.fields
            .get(name)
            .or_else(|| self.fields.get(si_name))
            .unwrap_or(&self.default)
    }
}

impl Layers {
    fn layer(&self, submodel: Submodel) -> &Layer {
        match submodel {
            Submodel::Stroke => &self.stroke,
            _ => &self.physics,
        }
    }

    fn validate(&self, path: &Path, files: &[TargetFile]) -> Result<(), LoadError> {
        for (name, layer) in [("physics", &self.physics), ("stroke", &self.stroke)] {
            for (field, bounds) in std::iter::once(("(default)", &layer.default)).chain(
                layer
                    .fields
                    .iter()
                    .map(|(name, bounds)| (name.as_str(), bounds)),
            ) {
                if ![bounds.abs, bounds.rel, bounds.f32_steps]
                    .into_iter()
                    .all(|value| value.is_finite() && value >= 0.0)
                {
                    return Err(LoadError::new(
                        path,
                        format!("invalid {name} bounds for {field}"),
                    ));
                }
            }
        }
        for target in files.iter().flat_map(|file| &file.targets) {
            let layer = self.layer(target.submodel);
            for (name, predicted) in target.earlier.iter().flat_map(|earlier| &earlier.predicted) {
                if predicted.as_number().is_none() {
                    continue;
                }
                let (si_name, _) = si_units(name);
                for source in &layer.bounds(name, &si_name).scale_from {
                    if input_scale(&target.inputs, source).is_none() {
                        return Err(LoadError::new(
                            path,
                            format!(
                                "target {:?} has no numeric input for scale_from {source:?}",
                                target.id
                            ),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn allowance(
        &self,
        submodel: Submodel,
        name: &str,
        inputs: &Quantities,
        earlier: f64,
        predicted: f64,
    ) -> f64 {
        let (si_name, to_si) = si_units(name);
        let bounds = self.layer(submodel).bounds(name, &si_name);
        let magnitude = earlier.abs().max(predicted.abs());
        let scale = bounds
            .scale_from
            .iter()
            .filter_map(|source| input_scale(inputs, source))
            .fold(1.0_f64.max(magnitude * to_si), f64::max);
        (bounds
            .abs
            .max(bounds.f32_steps * (1.0 / 8_388_608.0) * scale)
            / to_si)
            .max(magnitude * bounds.rel)
    }
}

fn input_scale(inputs: &Quantities, source: &str) -> Option<f64> {
    let key = source
        .strip_prefix("input.")
        .or_else(|| source.strip_prefix("inputs."))
        .unwrap_or(source);
    let input = inputs.get(key).and_then(Quantity::as_number)?;
    Some(input.abs() * si_units(key).1)
}

fn si_units(name: &str) -> (String, f64) {
    let (quantity, change) = name
        .strip_suffix("_change")
        .map_or((name, ""), |base| (base, "_change"));
    for (suffix, si_suffix, factor) in [
        ("_yd", "_m", 0.9144),
        ("_ft", "_m", 0.3048),
        ("_in", "_m", 0.0254),
        ("_mm", "_m", 0.001),
        ("_mph", "_mps", 0.44704),
        ("_deg", "_rad", std::f64::consts::PI / 180.0),
        ("_rpm", "_rad_s", std::f64::consts::TAU / 60.0),
    ] {
        if let Some(base) = quantity.strip_suffix(suffix) {
            return (format!("{base}{si_suffix}{change}"), factor);
        }
    }
    (name.into(), 1.0)
}
