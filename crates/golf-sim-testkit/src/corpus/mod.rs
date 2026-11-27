//! Runners for recorded corpus fixtures, selected by the fixture header.

mod course;
mod courses;
pub mod physics;
pub mod stroke;
mod world;

use std::{
    error::Error,
    path::{Path, PathBuf},
};

use golf_sim_base::{
    corpus,
    variant::{Dict, Variant},
};

use crate::tolerance::{self, CheckReport, DriftReport, Fixture};

use self::courses::Courses;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Runner {
    Variant,
    CourseGrids,
    CourseQueries,
    PhysicsValues,
    PhysicsFlight,
    PhysicsContact,
    PhysicsImpact,
    PhysicsFacade,
    PhysicsStrokes,
    WorldConstant,
    StrokeDelivery,
    StrokeFacade,
}

impl Runner {
    fn for_key(key: &str) -> Result<Self> {
        Ok(match key {
            "variant" | "variant/digests" => Self::Variant,
            "course/grids" => Self::CourseGrids,
            "course/queries" => Self::CourseQueries,
            "physics/values" => Self::PhysicsValues,
            "physics/flight" => Self::PhysicsFlight,
            "physics/contact" => Self::PhysicsContact,
            "physics/impact" => Self::PhysicsImpact,
            "physics/facade" => Self::PhysicsFacade,
            "physics/strokes" => Self::PhysicsStrokes,
            "world/constant" => Self::WorldConstant,
            "stroke/delivery" => Self::StrokeDelivery,
            "stroke/facade" => Self::StrokeFacade,
            _ => return Err(format!("no corpus runner for fixture key {key:?}").into()),
        })
    }
}

pub struct FixtureRun {
    runner: Runner,
    layer: String,
    path: PathBuf,
    header_course: Option<Dict>,
    courses: Courses,
    query_checks: course::QueryChecks,
    scratch: Option<generated::WithDepartures>,
}

impl FixtureRun {
    pub fn read(path: &Path, layer: Option<&str>, course_dirs: &[PathBuf]) -> Result<Self> {
        let envelope = corpus::read(path)?;
        let header = envelope.as_dict().expect("validated fixture header");
        let key = header
            .get("layer")
            .and_then(Variant::as_str)
            .expect("validated fixture key");
        let runner = Runner::for_key(key)?;
        let fixture_layer = key.split_once('/').map_or(key, |(layer, _)| layer);
        if let Some(layer) = layer
            && layer != fixture_layer
        {
            return Err(format!(
                "fixture key {key:?} belongs to layer {fixture_layer:?}, requested {layer:?}"
            )
            .into());
        }
        let testdata = testdata_for(path);
        let mut roots = course_dirs.to_vec();
        roots.push(testdata.join("courses"));
        let local_courses = PathBuf::from("testdata/courses");
        if !roots.contains(&local_courses) {
            roots.push(local_courses);
        }
        let header_course = child(header, "oracle")
            .and_then(|oracle| child(oracle, "course"))
            .cloned();
        let scratch = generated::WithDepartures::prepare(path, &testdata, runner, header)?;
        Ok(Self {
            runner,
            layer: fixture_layer.to_owned(),
            path: path.to_owned(),
            header_course,
            courses: Courses::new(roots),
            query_checks: course::QueryChecks::default(),
            scratch,
        })
    }

    pub fn check(self) -> Result<CheckReport> {
        self.check_observing(|_| {})
    }

    pub fn check_observing(mut self, mut observe: impl FnMut(&Dict)) -> Result<CheckReport> {
        let path = self.path_for_comparison();
        let layer = self.layer.clone();
        let report = tolerance::check(&path, &layer, |case| {
            observe(case);
            self.run(case, false)
        });
        if self.runner == Runner::CourseQueries {
            let bounds = self.query_checks.finish();
            let report = report?;
            bounds?;
            Ok(report)
        } else {
            report
        }
    }

    pub fn drift(mut self) -> Result<DriftReport> {
        let fixture = Fixture::read(&self.path_for_comparison())?;
        let layer = self.layer.clone();
        let report = fixture.drift(&layer, |case| self.run(case, true));
        if self.runner == Runner::CourseQueries {
            let _ = self.query_checks.finish();
        }
        report
    }

    fn path_for_comparison(&self) -> PathBuf {
        self.scratch
            .as_ref()
            .map_or_else(|| self.path.clone(), generated::WithDepartures::fixture)
    }

    fn run(&mut self, case: &Dict, drift: bool) -> Result<Variant> {
        let input = child(case, "input");
        let world = input.and_then(|input| child(input, "world"));
        let ground = world.or(input).and_then(|args| child(args, "ground"));
        let identity = ground
            .and_then(|ground| child(ground, "course"))
            .or_else(|| world.and_then(|world| child(world, "course")))
            .or_else(|| {
                input
                    .and_then(|input| child(input, "terrain"))
                    .and_then(|terrain| child(terrain, "course"))
            })
            .or_else(|| input.and_then(|input| child(input, "course")))
            .or_else(|| child(case, "course"));
        let course = identity
            .or(self.header_course.as_ref())
            .map(|identity| self.courses.open(identity))
            .transpose()?;
        let course = course.as_deref();

        Ok(match self.runner {
            Runner::Variant => tolerance::variant_snapshot(case)?,
            Runner::CourseGrids => {
                course::grids(course.ok_or("fixture has no course identity")?, case)?
            }
            Runner::CourseQueries => course::queries(
                course.ok_or("fixture has no course identity")?,
                case,
                &mut self.query_checks,
                drift,
            )?,
            Runner::PhysicsValues => physics::values::run_case(case, drift)?,
            Runner::PhysicsFlight => physics::flight::replay(case)?,
            Runner::PhysicsContact => physics::contact::run(case, drift)?,
            Runner::PhysicsImpact => physics::impact::compare(case, drift)?,
            Runner::PhysicsFacade => physics::facade(case, course),
            Runner::PhysicsStrokes => physics::strokes(case, course)?,
            Runner::WorldConstant => world::replay(case, course),
            Runner::StrokeDelivery => stroke::delivery::run(case),
            Runner::StrokeFacade => stroke::facade(case, course, drift),
        })
    }
}

fn child<'a>(dict: &'a Dict, name: &str) -> Option<&'a Dict> {
    dict.get(name).and_then(Variant::as_dict)
}

fn testdata_for(path: &Path) -> PathBuf {
    let location = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    location
        .ancestors()
        .skip(1)
        .find(|parent| parent.file_name().is_some_and(|name| name == "testdata"))
        .unwrap_or_else(|| Path::new("testdata"))
        .to_path_buf()
}

mod generated;
