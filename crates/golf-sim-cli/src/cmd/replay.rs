use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use clap::{Args, Subcommand};
use golf_sim::{
    base::{
        bundle::{Bundle, BundleCourse},
        container::Manifest,
        trace::NoProbe,
    },
    course::course::Course,
    stroke::repro::StrokeRepro,
};

use super::Outcome;

#[derive(Subcommand)]
pub enum Command {
    /// Replay a stroke bundle and report the first difference.
    Replay(BundleArgs),
}

#[derive(Args)]
pub struct BundleArgs {
    #[arg(value_name = "BUNDLE")]
    bundle: PathBuf,
    /// Course bundle to use. Otherwise searches GOLF_COURSES.
    #[arg(long, value_name = "DIR")]
    course: Option<PathBuf>,
    /// Stroke to replay. A stroke bundle contains only stroke 1.
    #[arg(long, value_name = "N")]
    stroke: Option<usize>,
}

pub fn run(command: Command) -> Outcome {
    let Command::Replay(args) = command;
    let (repro, course) = load(&args)?;
    repro.replay(&course, &mut NoProbe)?;
    println!("Stroke 1 replay matched bit for bit.");
    Ok(())
}

pub(super) fn load(
    args: &BundleArgs,
) -> Result<(StrokeRepro, Arc<Course>), Box<dyn std::error::Error>> {
    let bundle = Bundle::read(&args.bundle)?;
    let manifest = bundle.manifest;
    let course = find_course(&manifest.course, args.course.as_deref())?;
    let repro = StrokeRepro::from_bytes(&bundle.record)?;
    if manifest.course.id != repro.setup.course_id {
        return Err("bundle.json and record.bin name different courses".into());
    }
    if manifest.course.revision != repro.setup.course_revision {
        return Err(format!(
            "bundle.json names course revision {}, but record.bin needs {}",
            manifest.course.revision, repro.setup.course_revision
        )
        .into());
    }
    if manifest.focus.hole != Some(repro.setup.hole) || manifest.focus.stroke != 1 {
        return Err("bundle.json focus must name the hole and stroke 1 from record.bin".into());
    }
    if let Some(stroke) = args.stroke.filter(|&stroke| stroke != 1) {
        return Err(
            format!("stroke {stroke} is unavailable. This bundle contains only stroke 1").into(),
        );
    }
    Ok((repro, course))
}

fn find_course(
    expected: &BundleCourse,
    explicit: Option<&Path>,
) -> Result<Arc<Course>, Box<dyn std::error::Error>> {
    if let Some(dir) = explicit {
        return open_course(dir, expected);
    }
    let roots = env::var_os("GOLF_COURSES").map_or_else(
        || vec![PathBuf::from("testdata/courses")],
        |paths| env::split_paths(&paths).collect(),
    );

    let mut wrong_revision = None;
    for root in roots {
        let mut children: Vec<_> = fs::read_dir(&root)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect();
        children.sort();
        for dir in std::iter::once(root).chain(children) {
            let Ok(manifest) = Manifest::read(&dir) else {
                continue;
            };
            if manifest.course_id != expected.id {
                continue;
            }
            if manifest.course_revision == expected.revision {
                return open_course(&dir, expected);
            }
            wrong_revision.get_or_insert((dir, manifest.course_revision));
        }
    }
    if let Some((dir, actual)) = wrong_revision {
        return Err(revision_error(&dir, &expected.revision, &actual).into());
    }
    Err(format!(
        "course {:?} was not found. Use --course or set GOLF_COURSES",
        expected.id
    )
    .into())
}

fn open_course(
    dir: &Path,
    expected: &BundleCourse,
) -> Result<Arc<Course>, Box<dyn std::error::Error>> {
    let course = Course::open(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    if course.revision() != expected.revision {
        return Err(revision_error(dir, &expected.revision, course.revision()).into());
    }
    if course.data().id != expected.id {
        return Err(format!(
            "{} has course {:?}. The bundle needs {:?}",
            dir.display(),
            course.data().id,
            expected.id
        )
        .into());
    }
    Ok(course)
}

fn revision_error(dir: &Path, expected: &str, actual: &str) -> String {
    format!(
        "course revision mismatch at {}. Bundle expects {expected}, found {actual}",
        dir.display()
    )
}
