use golf_sim_base::variant::{Dict, Variant};
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use super::{Result, Runner, child};

pub(super) struct WithDepartures(PathBuf);

impl WithDepartures {
    pub(super) fn prepare(
        fixture: &Path,
        testdata: &Path,
        runner: Runner,
        header: &Dict,
    ) -> Result<Option<Self>> {
        if !matches!(runner, Runner::PhysicsFacade | Runner::PhysicsStrokes) {
            return Ok(None);
        }
        let Some(Variant::Array(cases)) = header.get("cases") else {
            unreachable!("validated fixture cases");
        };
        let ids: Vec<_> = cases
            .items
            .iter()
            .filter_map(|case| {
                let case = case.as_dict()?;
                let input = child(case, "input")?;
                ["conditions", "ground"]
                    .into_iter()
                    .filter_map(|name| child(input, name))
                    .any(|sampler| {
                        matches!(
                            sampler.get("kind").and_then(Variant::as_str),
                            Some("generated_air" | "generated_ground")
                        )
                    })
                    .then(|| {
                        case.get("id")
                            .and_then(Variant::as_str)
                            .expect("validated case id")
                    })
            })
            .collect();
        if ids.is_empty() {
            return Ok(None);
        }

        let scratch = Self::new()?;
        let dir = scratch.0.join("testdata");
        fs::create_dir(&dir)?;
        let limits = testdata.join("tolerances.toml");
        match fs::copy(limits, dir.join("tolerances.toml")) {
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let mut departures = match fs::read_to_string(testdata.join("departures.toml")) {
            Ok(contents) => contents,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                include_str!("../../departures.toml").to_owned()
            }
            Err(error) => return Err(error.into()),
        };
        for id in ids {
            use std::fmt::Write;
            let id = toml::Value::String(id.to_owned());
            writeln!(
                departures,
                "\n[[departure]]\nid = {id}\nlayer = 'physics'\nchange = 'requires the day generator'"
            )?;
        }
        fs::write(dir.join("departures.toml"), departures)?;
        fs::copy(fixture, scratch.fixture())?;
        Ok(Some(scratch))
    }

    fn new() -> Result<Self> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        loop {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("golf-corpus-{}-{id}", std::process::id()));
            match fs::create_dir(&dir) {
                Ok(()) => return Ok(Self(dir)),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
    }

    pub(super) fn fixture(&self) -> PathBuf {
        self.0.join("testdata/fixture.bin")
    }
}

impl Drop for WithDepartures {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
