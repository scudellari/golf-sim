//! Play and reproduce a single hole on a constant day.

pub mod advice;
mod apply;
pub mod prompt;
pub mod relief_ground;

use std::{collections::VecDeque, sync::Arc};

use golf_sim_base::{
    Error, Real,
    trace::{NoProbe, Trace},
};
use golf_sim_conditions::{lies::BallLie, world::World};
use golf_sim_course::course::Course;
use golf_sim_rules::{rulings::HoleRules, transitions};
use golf_sim_stroke::results::StrokeResult;

use crate::{
    action::Action,
    records::hole::{HoleRecord, Refusal},
    setup::HoleSetup,
};

#[derive(Debug)]
pub struct Step {
    pub stroke: Option<StrokeResult>,
}

#[derive(Debug)]
pub struct Hole {
    course: Arc<Course>,
    record: HoleRecord,
    rules: HoleRules,
    clock_s: Real,
    attempt: usize,
    ball_lie: BallLie,
    last_lie: BallLie,
    rest_lie: BallLie,
    original_lie: Option<BallLie>,
    trace_limit: usize,
    traces: VecDeque<Trace>,
}

impl Hole {
    pub fn start(course: Arc<Course>, setup: HoleSetup) -> Result<Self, Refusal> {
        Self::from_header(
            course,
            HoleRecord::begin(setup, HoleRecord::agent_driver("library")),
        )
    }

    /// Reproduce each entry in order, refusing at the first difference.
    pub fn resume(course: Arc<Course>, bytes: &[u8]) -> Result<Self, Refusal> {
        let mut record = HoleRecord::from_bytes(bytes)?;
        let entries = std::mem::take(&mut record.entries);
        let mut hole = Self::from_header(course, record)?;
        for entry in entries {
            hole.apply(entry, true)?;
        }
        Ok(hole)
    }

    fn from_header(course: Arc<Course>, record: HoleRecord) -> Result<Self, Refusal> {
        record.check_course(&course)?;
        let setup = &record.setup;
        let clock_s = setup.tee_time_s;
        if !clock_s.is_finite() {
            return Err(Error::Malformed {
                path: "setup.tee_time_s".into(),
                message: "expected a finite clock".into(),
            }
            .into());
        }
        let tee = setup.tee(&course).expect("checked tee");
        let world = setup.world(&course, clock_s).expect("checked course");
        let ball_lie = world.settle(tee.position, setup.seed.derive("tee"), &mut NoProbe);
        let rules = transitions::start_hole(ball_lie.spot).rules;
        Ok(Self {
            course,
            record,
            rules,
            clock_s,
            attempt: 0,
            ball_lie,
            last_lie: ball_lie,
            rest_lie: ball_lie,
            original_lie: None,
            trace_limit: 0,
            traces: VecDeque::new(),
        })
    }

    pub fn act(&mut self, action: Action) -> Result<Step, Refusal> {
        let entry = self.resolve(action)?;
        self.apply(entry, false)
    }

    pub fn record(&self) -> &HoleRecord {
        &self.record
    }

    pub fn rules(&self) -> &HoleRules {
        &self.rules
    }

    pub fn ball_lie(&self) -> Option<BallLie> {
        self.rules.ball().map(|_| self.ball_lie)
    }

    pub fn clock_s(&self) -> Real {
        self.clock_s
    }

    pub fn with_traces(mut self, limit: usize) -> Self {
        self.trace_limit = limit;
        self.traces.drain(..self.traces.len().saturating_sub(limit));
        self
    }

    /// Retained stroke traces, oldest first.
    pub fn traces(&self) -> &VecDeque<Trace> {
        &self.traces
    }

    fn world_at(&self, clock_s: Real) -> World<'_> {
        self.record
            .setup
            .world(&self.course, clock_s)
            .expect("checked course")
    }
}
