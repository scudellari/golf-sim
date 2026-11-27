//! Runners for recorded corpus fixtures, selected by the fixture header.

mod course;
mod courses;
mod foundation;
pub mod hole;
pub mod physics;
mod projection;
pub mod rules;
pub mod stroke;
pub mod swing;
mod world;

use std::{
    error::Error,
    path::{Path, PathBuf},
    sync::Arc,
};

use golf_sim_base::{
    corpus,
    variant::{Dict, Variant},
};

use golf_sim_course::course::Course;

use crate::tolerance::{self, CheckReport, DriftReport, Fixture};

use self::courses::Courses;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Runner {
    Variant,
    MathPrims,
    SeedStreams,
    GeometryCases,
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
    SwingStreams,
    RulesTransitions,
    RulesRelief,
    RecordsHoles,
    HoleAdvice,
    HoleObserve,
}

impl Runner {
    fn projects(self) -> bool {
        matches!(
            self,
            Self::MathPrims
                | Self::SeedStreams
                | Self::GeometryCases
                | Self::RulesRelief
                | Self::RecordsHoles
                | Self::HoleAdvice
                | Self::HoleObserve
        )
    }

    fn for_key(key: &str) -> Result<Self> {
        Ok(match key {
            "variant" | "variant/digests" => Self::Variant,
            "math/prims" => Self::MathPrims,
            "seed/streams" => Self::SeedStreams,
            "geometry/cases" => Self::GeometryCases,
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
            "swing/streams" => Self::SwingStreams,
            "rules/transitions" => Self::RulesTransitions,
            "rules/relief" => Self::RulesRelief,
            "records/holes" => Self::RecordsHoles,
            "hole/advice" => Self::HoleAdvice,
            "hole/observe" => Self::HoleObserve,
            _ => return Err(format!("no corpus runner for fixture key {key:?}").into()),
        })
    }
}

pub struct FixtureRun {
    runner: Runner,
    layer: String,
    key: String,
    path: PathBuf,
    header_course: Option<Dict>,
    courses: Courses,
    testdata: PathBuf,
    header: Dict,
    query_checks: course::QueryChecks,
    scratch: Option<generated::WithDepartures>,
}

impl FixtureRun {
    pub fn read(path: &Path, layer: Option<&str>, course_dirs: &[PathBuf]) -> Result<Self> {
        let Variant::Dict(header) = corpus::read(path)? else {
            unreachable!("validated fixture header");
        };
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
        let header_course = child(&header, "oracle")
            .and_then(|oracle| child(oracle, "course"))
            .cloned();
        let scratch = generated::WithDepartures::prepare(&testdata, runner, &header)?;
        Ok(Self {
            runner,
            layer: fixture_layer.to_owned(),
            key: key.to_owned(),
            path: path.to_owned(),
            header_course,
            courses: Courses::new(roots),
            testdata,
            header,
            query_checks: course::QueryChecks::default(),
            scratch,
        })
    }

    pub fn check(self) -> Result<CheckReport> {
        self.check_observing(|_| {})
    }

    pub fn check_observing(mut self, mut observe: impl FnMut(&Dict)) -> Result<CheckReport> {
        let header = std::mem::take(&mut self.header);
        if self.runner.projects() {
            let mut projected = self.project(&header, false)?;
            return projected.fixture().check(&self.layer, |case| {
                observe(case);
                Ok::<_, std::convert::Infallible>(projected.actual(case))
            });
        }
        let fixture = self.comparison(header);
        let layer = self.layer.clone();
        let report = fixture.check(&layer, |case| {
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
        let header = std::mem::take(&mut self.header);
        if self.runner.projects() {
            let mut projected = self.project(&header, true)?;
            return projected.fixture().drift(&self.layer, |case| {
                Ok::<_, std::convert::Infallible>(projected.actual(case))
            });
        }
        let fixture = self.comparison(header);
        let layer = self.layer.clone();
        let report = fixture.drift(&layer, |case| self.run(case, true));
        if self.runner == Runner::CourseQueries {
            let _ = self.query_checks.finish();
        }
        report
    }

    fn comparison(&self, header: Dict) -> Fixture {
        let testdata = self.scratch.as_ref().map_or_else(
            || self.testdata.clone(),
            generated::WithDepartures::testdata,
        );
        Fixture::from_header(header, testdata)
    }

    fn project(&mut self, header: &Dict, drift: bool) -> Result<projection::Projection> {
        let (pairs, tolerance_layer) = match self.runner {
            Runner::MathPrims | Runner::SeedStreams | Runner::GeometryCases => (
                projection::rows(header)
                    .map(|case| foundation::project(self.runner, case, drift))
                    .collect::<Result<_>>()?,
                None,
            ),
            Runner::RulesRelief => (rules::project(self, header, drift)?, Some("relief")),
            Runner::RecordsHoles => (hole::records(self, header, drift)?, Some("hole")),
            Runner::HoleAdvice | Runner::HoleObserve => {
                (hole::addresses(self, header, drift)?, None)
            }
            _ => unreachable!("runner uses unprojected output"),
        };
        let departures = if self.runner == Runner::RulesRelief {
            projection::rows(header)
                .filter(|case| rules::uses_generated_stroke(case))
                .map(|case| (projection::id(case), "requires the day generator"))
                .collect()
        } else {
            Vec::new()
        };
        projection::Projection::prepare(header, &self.testdata, tolerance_layer, pairs, departures)
    }

    fn companion(&self, key: &str) -> Result<Variant> {
        let file = self.path.file_name().ok_or("fixture has no filename")?;
        let candidate = self
            .path
            .parent()
            .filter(|dir| dir.ends_with(&self.key))
            .and_then(|dir| dir.parent()?.parent())
            .map(|root| root.join(key));
        let path = candidate
            .into_iter()
            .flat_map(|dir| [dir.join(file), dir.join("fixture.bin")])
            .find(|path| path.is_file())
            .unwrap_or_else(|| self.testdata.join(key).join("fixture.bin"));
        let companion =
            corpus::read(&path).map_err(|error| format!("companion {key:?}: {error}"))?;
        if companion
            .as_dict()
            .and_then(|header| header.get("layer"))
            .and_then(Variant::as_str)
            != Some(key)
        {
            return Err(
                format!("{}: expected companion fixture key {key:?}", path.display()).into(),
            );
        }
        Ok(companion)
    }

    fn require_course(&mut self, case: &Dict) -> Result<Arc<Course>> {
        self.resolve_course(case)?
            .ok_or_else(|| "fixture has no course identity".into())
    }

    fn resolve_course(&mut self, case: &Dict) -> Result<Option<Arc<Course>>> {
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
        Ok(identity
            .or(self.header_course.as_ref())
            .map(|identity| self.courses.open(identity))
            .transpose()?)
    }

    fn run(&mut self, case: &Dict, drift: bool) -> Result<Variant> {
        let resolved = self.resolve_course(case)?;
        let course = resolved.as_deref();

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
            Runner::SwingStreams => swing::run(case),
            Runner::RulesTransitions => rules::run(case, drift)?,
            Runner::MathPrims
            | Runner::SeedStreams
            | Runner::GeometryCases
            | Runner::RulesRelief
            | Runner::RecordsHoles
            | Runner::HoleAdvice
            | Runner::HoleObserve => {
                unreachable!("runner uses projected output")
            }
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
