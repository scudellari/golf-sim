use std::{
    fmt::Write,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use golf_sim_base::variant::{self, Array, Dict, Variant};

use super::Result;

pub(super) type Pair = (Variant, Variant);

pub(super) fn rows(header: &Dict) -> impl Iterator<Item = &Dict> {
    let Some(Variant::Array(cases)) = header.get("cases") else {
        unreachable!("validated fixture cases");
    };
    cases.items.iter().map(|case| case.as_dict().unwrap())
}

pub(super) fn id(case: &Dict) -> &str {
    case.get("id").and_then(Variant::as_str).unwrap()
}

pub(super) struct Projection {
    dir: PathBuf,
    actual: Vec<Variant>,
}

impl Projection {
    pub(super) fn write(
        header: &Dict,
        testdata: &Path,
        tolerance_layer: Option<&str>,
        pairs: impl IntoIterator<Item = Pair>,
        departures: Vec<(&str, &str)>,
    ) -> Result<Self> {
        let dir = Self::scratch_dir()?;
        let mut projection = Self {
            dir,
            actual: Vec::new(),
        };
        fs::create_dir(projection.dir.join("testdata"))?;
        let mut cases = Vec::new();
        let mut pairs = pairs.into_iter();
        for case in rows(header) {
            let mut case = case.entries().to_vec();
            let (expected, actual) = pairs.next().ok_or("missing projected case")?;
            case.retain(|(key, _)| {
                !matches!(key.as_str(), Some("output" | "corpus_projection_index"))
            });
            case.push(("output".into(), expected));
            case.push((
                "corpus_projection_index".into(),
                (projection.actual.len() as i64).into(),
            ));
            cases.push(Variant::Dict(Dict::new(case)));
            projection.actual.push(actual);
        }
        let mut fixture = header.clone();
        *fixture.get_mut("cases").unwrap() = Variant::Array(Array::new(cases));
        let body = variant::encode(&Variant::Dict(fixture))?;
        let bytes = [
            b"GSCORP01".as_slice(),
            &u32::try_from(body.len())?.to_le_bytes(),
            &body,
        ]
        .concat();
        fs::write(projection.path(), bytes)?;

        let config = testdata.join("tolerances.toml");
        let limits = match fs::read_to_string(config) {
            Ok(limits) => limits,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                include_str!("../../tolerances.toml").to_owned()
            }
            Err(error) => return Err(error.into()),
        };
        let limits = if let Some(layer) = tolerance_layer {
            let key = header.get("layer").and_then(Variant::as_str).unwrap();
            let mut table: toml::Table = toml::from_str(&limits)?;
            if !table.contains_key(key) {
                let row = table
                    .get(layer)
                    .ok_or_else(|| format!("no tolerances for {layer:?}"))?;
                table.insert(key.into(), row.clone());
            }
            toml::to_string(&table)?
        } else {
            limits
        };
        fs::write(projection.dir.join("testdata/tolerances.toml"), limits)?;
        let mut listed = match fs::read_to_string(testdata.join("departures.toml")) {
            Ok(listed) => listed,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                include_str!("../../departures.toml").to_owned()
            }
            Err(error) => return Err(error.into()),
        };
        let key = header.get("layer").and_then(Variant::as_str).unwrap();
        let layer = key.split_once('/').map_or(key, |(layer, _)| layer);
        let quoted = |value: &str| toml::Value::String(value.to_owned());
        for (id, reason) in departures {
            writeln!(
                listed,
                "\n[[departure]]\nid = {}\nlayer = {}\nchange = {}",
                quoted(id),
                quoted(layer),
                quoted(reason),
            )?;
        }
        fs::write(projection.dir.join("testdata/departures.toml"), listed)?;
        Ok(projection)
    }

    pub(super) fn actual(&self, case: &Dict) -> Variant {
        let Some(Variant::Int(index)) = case.get("corpus_projection_index") else {
            unreachable!("projected case index");
        };
        self.actual[index.value() as usize].clone()
    }

    pub(super) fn path(&self) -> PathBuf {
        self.dir.join("testdata/fixture.bin")
    }

    fn scratch_dir() -> Result<PathBuf> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        loop {
            let seq = NEXT.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("golf-projection-{}-{seq}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(path),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
    }
}

impl Drop for Projection {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}
