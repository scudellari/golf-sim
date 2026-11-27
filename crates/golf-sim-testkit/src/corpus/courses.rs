use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use golf_sim_base::{
    container::Manifest,
    variant::{Dict, Variant},
};
use golf_sim_course::course::Course;

pub(super) struct Courses {
    bundles: BTreeMap<(String, String), PathBuf>,
    explicit_revisions: BTreeMap<String, BTreeSet<String>>,
    inventory_error: Option<String>,
    opened: BTreeMap<(String, String), Arc<Course>>,
}

impl Courses {
    pub(super) fn new(roots: Vec<PathBuf>, explicit: &[PathBuf]) -> Self {
        let mut bundles = BTreeMap::new();
        let mut explicit_revisions = BTreeMap::<String, BTreeSet<String>>::new();
        let mut errors = BTreeSet::new();
        for root in &roots {
            let mut inspect = |dir: &Path| match Manifest::read(dir) {
                Ok(manifest) => {
                    if explicit.contains(root) {
                        explicit_revisions
                            .entry(manifest.course_id.clone())
                            .or_default()
                            .insert(manifest.course_revision.clone());
                    }
                    bundles
                        .entry((manifest.course_id, manifest.course_revision))
                        .or_insert_with(|| dir.to_owned());
                }
                Err(error) => {
                    errors.insert(format!("{}: {error}", dir.join("manifest.json").display()));
                }
            };
            match fs::symlink_metadata(root.join("manifest.json")) {
                Ok(_) => inspect(root),
                Err(error) if error.kind() == io::ErrorKind::NotFound => match bundle_dirs(root) {
                    Ok(dirs) => dirs.iter().map(PathBuf::as_path).for_each(inspect),
                    Err(error) => {
                        errors.insert(format!("{}: {error}", root.display()));
                    }
                },
                Err(error) => {
                    errors.insert(format!("{}: {error}", root.join("manifest.json").display()));
                }
            }
        }
        let inventory_error = (!errors.is_empty()).then(|| {
            format!(
                "course inventory could not be read: {}",
                errors.into_iter().collect::<Vec<_>>().join("; ")
            )
        });
        Self {
            bundles,
            explicit_revisions,
            inventory_error,
            opened: BTreeMap::new(),
        }
    }

    pub(super) fn open(&mut self, identity: &Dict) -> Result<Option<Arc<Course>>, String> {
        let field = |name| {
            identity
                .get(name)
                .and_then(Variant::as_str)
                .ok_or_else(|| format!("expected course {name}"))
        };
        let id = field("id")?;
        let revision = field("revision")?;
        let key = (id.to_owned(), revision.to_owned());
        if let Some(course) = self.opened.get(&key) {
            return Ok(Some(Arc::clone(course)));
        }
        let Some(dir) = self.bundles.get(&key) else {
            return match &self.inventory_error {
                Some(error) => Err(error.clone()),
                None => Ok(None),
            };
        };
        let course = Course::open(dir)
            .map_err(|error| format!("course {id:?} revision {revision:?}: {error}"))?;
        self.opened.insert(key, Arc::clone(&course));
        Ok(Some(course))
    }

    pub(super) fn is_modified_copy(&self, id: &str, revision: &str) -> bool {
        self.inventory_error.is_none()
            && self
                .explicit_revisions
                .get(id)
                .is_some_and(|revisions| !revisions.contains(revision))
    }
}

fn bundle_dirs(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut children = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() || (entry.file_type()?.is_symlink() && entry.path().is_dir())
        {
            children.push(entry.path());
        }
    }
    children.sort();
    Ok(children)
}

#[cfg(test)]
mod tests {
    use super::*;
    use golf_sim_base::digest::object;

    #[test]
    fn explicit_directory_distinguishes_a_modified_revision_from_an_unknown_course() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/courses");
        let manifest = Manifest::read(&root.join("test-course")).unwrap();
        let mut courses = Courses::new(vec![root.clone()], &[root]);
        let request =
            |id: &str, revision: &str| object([("id", id.into()), ("revision", revision.into())]);
        let exact = request(&manifest.course_id, &manifest.course_revision);
        let edited = request(&manifest.course_id, "modified");
        let missing = request("unknown-synthetic-course", "modified");
        assert!(courses.open(exact.as_dict().unwrap()).unwrap().is_some());
        assert!(courses.open(edited.as_dict().unwrap()).unwrap().is_none());
        assert!(courses.is_modified_copy(&manifest.course_id, "modified"));
        assert!(!courses.is_modified_copy(&manifest.course_id, &manifest.course_revision));
        assert!(courses.open(missing.as_dict().unwrap()).unwrap().is_none());
        assert!(!courses.is_modified_copy("unknown-synthetic-course", "modified"));
    }
}
