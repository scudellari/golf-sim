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
    digest::sha256_hex,
    variant::{Dict, Variant},
};

use golf_sim_conditions::grounds::CourseGround;
use golf_sim_course::course::Course;

use crate::tolerance::{
    self, CaseOutcome, CheckReport, Departures, DriftReport, Fixture, PrecisionDepartureRule,
};

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
    stroke_models: physics::models::Versions,
    precision_departures: Vec<PrecisionDepartureRule>,
    stroke_limits: Option<tolerance::Layer>,
    tunable_departures: Option<physics::values::TunableDepartures>,
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
        if roots.is_empty() {
            roots.push(testdata.join("courses"));
            let local_courses = PathBuf::from("testdata/courses");
            if !roots.contains(&local_courses) {
                roots.push(local_courses);
            }
        }
        let header_course = child(&header, "oracle")
            .and_then(|oracle| child(oracle, "course"))
            .cloned();
        let scratch = generated::WithDepartures::prepare(&testdata, runner, &header)?;
        let tunable_departures = (runner == Runner::PhysicsValues)
            .then(|| {
                physics::values::TunableDepartures::new(
                    &header,
                    Departures::load(&testdata)?.field_departure,
                )
            })
            .transpose()?;
        let stroke_models = physics::models::Versions::for_fixture(&header);
        let stroke_limits = matches!(
            runner,
            Runner::PhysicsStrokes | Runner::PhysicsFacade | Runner::StrokeFacade
        )
        .then(|| tolerance::Layer::for_fixture(&testdata, key))
        .transpose()?;
        let precision_departures = if stroke_limits.is_some() {
            Departures::load(&testdata)?
                .precision_departure
                .into_iter()
                .filter(|rule| rule.fixture == key)
                .collect()
        } else {
            Vec::new()
        };
        Ok(Self {
            runner,
            layer: fixture_layer.to_owned(),
            key: key.to_owned(),
            path: path.to_owned(),
            header_course,
            courses: Courses::new(roots, course_dirs),
            testdata,
            header,
            query_checks: course::QueryChecks::default(),
            scratch,
            tunable_departures,
            stroke_models,
            precision_departures,
            stroke_limits,
        })
    }

    /// Cases from the loaded fixture, before projection or departure filtering.
    pub fn cases(&self) -> impl ExactSizeIterator<Item = &Dict> {
        projection::rows(&self.header)
    }

    pub fn check(self) -> Result<CheckReport> {
        self.check_observing(|_| {})
    }

    pub fn check_observing(mut self, mut observe: impl FnMut(&Dict)) -> Result<CheckReport> {
        let header = std::mem::take(&mut self.header);
        if self.runner == Runner::Variant {
            return self.comparison(header).check(&self.layer, |case| {
                observe(case);
                tolerance::variant_snapshot(case)
            });
        }
        if self.runner.projects() {
            let mut projected = self.project(&header, false)?;
            return projected.fixture().check(&self.layer, |case| {
                observe(case);
                Ok::<_, std::convert::Infallible>(projected.actual(case))
            });
        }
        let prepared = self.primary_course(&header);
        let prepared = prepared
            .as_deref()
            .map(|course| PreparedCourse::new(course, self.runner));
        let fixture = self.comparison(header);
        let layer = self.layer.clone();
        let report = fixture.check_cases(&layer, |case| {
            observe(case);
            self.run(case, false, prepared.as_ref())
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
        let prepared = self.primary_course(&header);
        let prepared = prepared
            .as_deref()
            .map(|course| PreparedCourse::new(course, self.runner));
        let fixture = self.comparison(header);
        let layer = self.layer.clone();
        let report = fixture.drift_cases(&layer, |case| self.run(case, true, prepared.as_ref()));
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
        // Recorded actions can diverge after a model change, before the projected values are compared.
        let listed = Departures::load(&self.testdata)?;
        let listed = listed.cases_for(&self.key);
        let replay = (!listed.is_empty()).then(|| projection::replay_header(header, &listed));
        let replay = replay.as_ref().unwrap_or(header);
        let (pairs, tolerance_layer) = match self.runner {
            Runner::MathPrims | Runner::SeedStreams | Runner::GeometryCases => (
                projection::rows(replay)
                    .map(|case| foundation::project(self.runner, case, drift))
                    .collect::<Result<_>>()?,
                None,
            ),
            Runner::RulesRelief => (rules::project(self, replay, drift)?, Some("relief")),
            Runner::RecordsHoles => (hole::records(self, replay, drift)?, Some("hole")),
            Runner::HoleAdvice | Runner::HoleObserve => {
                (hole::addresses(self, replay, drift)?, None)
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
        projection::Projection::prepare(
            header,
            &self.testdata,
            tolerance_layer,
            pairs,
            departures,
            &listed,
        )
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
        let Some(identity) = course_identity(case).or(self.header_course.as_ref()) else {
            return Ok(None);
        };
        self.courses
            .open(identity)?
            .map(Some)
            .ok_or_else(|| missing_course(identity).into())
    }

    fn primary_course(&mut self, header: &Dict) -> Option<Arc<Course>> {
        if !matches!(
            self.runner,
            Runner::CourseQueries | Runner::WorldConstant | Runner::StrokeFacade
        ) {
            return None;
        }
        let identity = self
            .header_course
            .as_ref()
            .or_else(|| projection::rows(header).find_map(course_identity))?;
        self.courses.open(identity).ok().flatten()
    }

    fn run(
        &mut self,
        case: &Dict,
        drift: bool,
        prepared: Option<&PreparedCourse<'_>>,
    ) -> Result<CaseOutcome> {
        if let Some(reason) = self.unsupported_sampler(case) {
            return Ok(CaseOutcome::Refused(reason));
        }
        let resolved = if let Some(identity) = course_identity(case).or(self.header_course.as_ref())
        {
            let course = match self.courses.open(identity) {
                Ok(Some(course)) => course,
                Err(reason) => return Ok(CaseOutcome::Refused(reason)),
                Ok(None) => {
                    let id = identity.get("id").and_then(Variant::as_str).unwrap();
                    let revision = identity.get("revision").and_then(Variant::as_str).unwrap();
                    return Ok(if self.courses.is_modified_copy(id, revision) {
                        CaseOutcome::ModifiedCourse {
                            course: id.to_owned(),
                            revision: revision.to_owned(),
                        }
                    } else {
                        CaseOutcome::Refused(missing_course(identity))
                    });
                }
            };
            Some(course)
        } else {
            None
        };
        let course = resolved.as_deref();
        let prepared = prepared
            .filter(|prepared| course.is_some_and(|course| std::ptr::eq(course, prepared.course)));
        let shared_ground = prepared.and_then(|prepared| prepared.ground.as_ref());

        if let Some(rule) = self.precision_rule(case) {
            return self.run_precision(case, course, rule, drift);
        }

        Ok(match self.runner {
            Runner::Variant => tolerance::variant_snapshot(case)?,
            Runner::CourseGrids => {
                course::grids(course.ok_or("fixture has no course identity")?, case)?
            }
            Runner::CourseQueries => {
                let course = course.ok_or("fixture has no course identity")?;
                let fallback;
                let queries = if let Some(queries) = prepared.and_then(|p| p.queries.as_ref()) {
                    queries
                } else {
                    fallback = course::Queries::new(course);
                    &fallback
                };
                course::queries(queries, case, &mut self.query_checks, drift)?
            }
            Runner::PhysicsValues => {
                return self
                    .tunable_departures
                    .as_ref()
                    .expect("loaded physics values comparison")
                    .run(case, drift);
            }
            Runner::PhysicsFlight => physics::flight::replay(case)?,
            Runner::PhysicsContact => physics::contact::run(case, drift)?,
            Runner::PhysicsImpact => physics::impact::compare(case, course, drift)?,
            Runner::PhysicsFacade | Runner::PhysicsStrokes | Runner::StrokeFacade => {
                return self.stroke_outcome(case, course, drift);
            }
            Runner::WorldConstant => world::replay(case, course, shared_ground, drift)?,
            Runner::StrokeDelivery => stroke::delivery::run(case),
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
        }
        .into())
    }

    fn precision_rule(&self, case: &Dict) -> Option<&PrecisionDepartureRule> {
        let call = case.get("call").and_then(Variant::as_str)?;
        if !matches!(
            call,
            "shot_integrator.run"
                | "golf_sim.simulate_launch"
                | "golf_sim.simulate_shot"
                | "stroke.play"
        ) || self.precision_departures.is_empty()
        {
            return None;
        }
        let digest = sha256_hex(case.get("id").and_then(Variant::as_str)?.as_bytes());
        self.precision_departures
            .iter()
            .find(|rule| rule.call == call && rule.ids_sha256.contains(&digest))
    }

    fn stroke_outcome(
        &self,
        case: &Dict,
        course: Option<&Course>,
        drift: bool,
    ) -> Result<CaseOutcome> {
        Ok(match self.runner {
            Runner::PhysicsStrokes => physics::strokes(case, course, self.stroke_models, drift)?,
            Runner::PhysicsFacade => self
                .stroke_models
                .run(|probe| physics::facade(case, course, probe, drift))?,
            Runner::StrokeFacade => stroke::facade_with_model(
                case,
                course,
                drift,
                self.stroke_models,
                self.stroke_limits.as_ref().unwrap(),
            )?,
            _ => unreachable!("not a stroke runner"),
        })
    }

    fn run_precision(
        &self,
        case: &Dict,
        course: Option<&Course>,
        rule: &PrecisionDepartureRule,
        drift: bool,
    ) -> Result<CaseOutcome> {
        if matches!(rule.call.as_str(), "golf_sim.simulate_shot" | "stroke.play") {
            return self.run_recorded_launch(case, course, drift);
        }
        let raw = self.stroke_outcome(case, course, true)?;
        let compared = self.stroke_outcome(case, course, false);
        let CaseOutcome::Compared { actual: raw, .. } = raw else {
            return Err(
                "precision departure requires a stroke without a changed model response".into(),
            );
        };
        let step_scale = if self.runner == Runner::PhysicsStrokes {
            match child(case, "input").and_then(|input| input.get("step_scale")) {
                Some(Variant::Float(value)) => value.value(),
                Some(Variant::Int(value)) => value.value() as f64,
                _ => return Err("missing integrator step scale".into()),
            }
        } else {
            1.0
        };
        let CaseOutcome::Compared { actual, .. } = compared? else {
            return Err(
                "precision departure requires a stroke without a changed model response".into(),
            );
        };
        let expected = case.get("output").ok_or("missing stroke output")?;
        let rounding = physics::playback::check_initial_rounding(expected, &raw);
        let limits = self.stroke_limits.as_ref().unwrap();
        let proof = physics::playback::verify_response(
            expected,
            &actual,
            limits,
            step_scale,
            |checkpoint| physics::replay_checkpoint(case, course, None, checkpoint),
        )
        .map_err(|failure| format!("replay from recorded launch: {failure}"))?;
        let Some(proof) = proof else {
            return Ok(actual.into());
        };
        let first =
            rounding.map_err(|failure| format!("replay from recorded launch passed; {failure}"))?;
        if proof.threshold != rule.threshold {
            return Err(format!(
                "expected verified {} at the first over-bound checkpoint, found {}: {}",
                rule.threshold, proof.threshold, proof.first_over_bound,
            )
            .into());
        }
        Ok(CaseOutcome::PrecisionDeparture {
            actual: if drift { raw } else { actual },
            change: format!(
                "float rounding at {}; first difference {first}; first over-bound {}",
                rule.threshold, proof.first_over_bound
            ),
        })
    }

    fn run_recorded_launch(
        &self,
        case: &Dict,
        course: Option<&Course>,
        drift: bool,
    ) -> Result<CaseOutcome> {
        let CaseOutcome::Compared { actual, .. } = self.stroke_outcome(case, course, true)? else {
            return Err("recorded-launch departure requires an unchanged strike model".into());
        };
        let limits = self.stroke_limits.as_ref().unwrap();
        let strike = match self.runner {
            Runner::PhysicsFacade => physics::verify_delivery(case, &actual, course, limits),
            Runner::StrokeFacade => stroke::verify_delivery(case, &actual, course, limits),
            _ => return Err("recorded-launch departure requires a delivered shot".into()),
        };
        let expected = case.get("output").ok_or("missing stroke output")?;
        let launch = physics::playback::playback(expected)
            .and_then(Variant::as_dict)
            .and_then(|shot| child(shot, "launch"))
            .map(physics::recorded_launch)
            .ok_or("missing recorded launch")?;
        let compared = |outcome| match outcome {
            CaseOutcome::Compared { actual, .. } => Ok(actual),
            _ => Err("recorded-launch replay reached a changed model response".into()),
        };
        let replay = |checkpoint| match self.runner {
            Runner::PhysicsFacade => match checkpoint {
                None => compared(self.stroke_models.run(|probe| {
                    physics::facade_with_launch(case, course, probe, false, Some(&launch))
                })?),
                Some(checkpoint) => {
                    physics::replay_checkpoint(case, course, Some(&launch), checkpoint)
                }
            },
            Runner::StrokeFacade => compared(stroke::replay_from_launch(
                case,
                course,
                self.stroke_models,
                &launch,
                checkpoint,
            )?),
            _ => unreachable!(),
        };
        let replayed =
            replay(None).map_err(|error| format!("replay from recorded launch: {error}"))?;
        physics::playback::verify_response(expected, &replayed, limits, 1.0, |checkpoint| {
            replay(Some(checkpoint))
        })
        .map_err(|failure| format!("replay from recorded launch: {failure}"))?;
        strike
            .map_err(|failure| format!("strike: {failure}; replay from recorded launch passed"))?;
        Ok(CaseOutcome::PrecisionDeparture {
            actual: if drift { actual } else { replayed },
            change: "launch rounding at the strike, flight verified from the recorded launch"
                .into(),
        })
    }

    fn unsupported_sampler(&self, case: &Dict) -> Option<String> {
        let input = child(case, "input")?;
        if self.runner == Runner::PhysicsImpact {
            return physics::impact::unsupported_ground(input);
        }
        let (air, ground, ground_kinds): (_, _, &[_]) = match self.runner {
            Runner::WorldConstant | Runner::StrokeFacade => {
                let world = child(input, "world")?;
                (
                    child(world, "air"),
                    child(world, "ground"),
                    &["course_ground", "practice_ground", "plane"],
                )
            }
            Runner::PhysicsFacade | Runner::PhysicsStrokes => (
                child(input, "conditions"),
                child(input, "ground"),
                &["course_ground", "practice_ground", "plane"],
            ),
            _ => return None,
        };
        let ground = if matches!(self.runner, Runner::PhysicsFacade | Runner::PhysicsStrokes) {
            ground.map(unwrapped_ground)
        } else {
            ground
        };
        for (sampler, name, supported) in [
            (air, "air", &["constant_air"][..]),
            (ground, "ground", ground_kinds),
        ] {
            if let Some(kind) = sampler
                .and_then(|sampler| sampler.get("kind"))
                .and_then(Variant::as_str)
                && !supported.contains(&kind)
            {
                return Some(format!("unsupported {name} sampler {kind:?}"));
            }
        }
        None
    }
}

struct PreparedCourse<'a> {
    course: &'a Course,
    queries: Option<course::Queries<'a>>,
    ground: Option<CourseGround<'a>>,
}

impl<'a> PreparedCourse<'a> {
    fn new(course: &'a Course, runner: Runner) -> Self {
        Self {
            course,
            queries: (runner == Runner::CourseQueries).then(|| course::Queries::new(course)),
            ground: (runner == Runner::WorldConstant)
                .then(|| CourseGround::new(course, 0.5, 0.3, 0.0)),
        }
    }
}

fn missing_course(identity: &Dict) -> String {
    let field = |key| {
        identity
            .get(key)
            .and_then(Variant::as_str)
            .unwrap_or_default()
    };
    format!(
        "course {:?} revision {:?} was not found (use --course-dir)",
        field("id"),
        field("revision")
    )
}

fn unwrapped_ground(mut ground: &Dict) -> &Dict {
    while ground.get("kind").and_then(Variant::as_str) == Some("mown_ground") {
        let Some(inner) = child(ground, "ground") else {
            break;
        };
        ground = inner;
    }
    ground
}

fn course_identity(case: &Dict) -> Option<&Dict> {
    let input = child(case, "input");
    let world = input.and_then(|input| child(input, "world"));
    let ground = world.or(input).and_then(|args| child(args, "ground"));
    ground
        .and_then(|ground| child(unwrapped_ground(ground), "course"))
        .or_else(|| world.and_then(|world| child(world, "course")))
        .or_else(|| {
            input
                .and_then(|input| child(input, "terrain"))
                .and_then(|terrain| child(terrain, "course"))
        })
        .or_else(|| input.and_then(|input| child(input, "course")))
        .or_else(|| child(case, "course"))
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
