use std::{collections::BTreeMap, fs, path::PathBuf, sync::Arc};

use golf_sim_base::{
    container::Manifest,
    variant::{Dict, Variant},
};
use golf_sim_course::course::Course;

pub(super) struct Courses {
    roots: Vec<PathBuf>,
    opened: BTreeMap<(String, String), Arc<Course>>,
}

impl Courses {
    pub(super) fn new(roots: Vec<PathBuf>) -> Self {
        Self {
            roots,
            opened: BTreeMap::new(),
        }
    }

    pub(super) fn open(&mut self, identity: &Dict) -> Result<Arc<Course>, String> {
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
            return Ok(Arc::clone(course));
        }

        let missing =
            || format!("course {id:?} revision {revision:?} was not found (use --course-dir)");
        for root in &self.roots {
            let mut children: Vec<_> = fs::read_dir(root)
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .collect();
            children.sort();
            for dir in std::iter::once(root.clone()).chain(children) {
                let Ok(manifest) = Manifest::read(&dir) else {
                    continue;
                };
                if manifest.course_id == id && manifest.course_revision == revision {
                    let course = Course::open(&dir)
                        .map_err(|error| format!("course {id:?} revision {revision:?}: {error}"))?;
                    self.opened.insert(key, Arc::clone(&course));
                    return Ok(course);
                }
            }
        }
        Err(missing())
    }
}
