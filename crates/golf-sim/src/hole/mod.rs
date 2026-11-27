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
use golf_sim_stroke::{
    delivery::DeliveryInput,
    results::StrokeResult,
    stroke::{self, StrokeInput},
};

use crate::{
    action::Action,
    records::hole::{Entry, HoleRecord, Refusal, StrokeFacts},
    setup::{HoleSetup, Mode},
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

    pub fn act(&mut self, action: Action) -> Result<Step, prompt::ActionRefusal> {
        self.check_action_prompt(&action)?;
        let entry = self.resolve(action)?;
        Ok(self.apply(entry, false)?)
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

    pub(crate) fn course(&self) -> &Course {
        &self.course
    }

    pub(crate) fn last_address_lie(&self) -> BallLie {
        self.last_lie
    }

    fn stroke_facts(&self) -> impl Iterator<Item = &StrokeFacts> {
        self.record
            .entries
            .iter()
            .rev()
            .filter_map(|entry| match entry {
                Entry::Stroke(stroke) => Some(&stroke.facts),
                Entry::Decision(_) => None,
            })
    }

    pub(crate) fn last_address(&self) -> Option<&StrokeFacts> {
        self.stroke_facts().next()
    }

    pub(crate) fn advice_in(&self, mode: Mode) -> Option<advice::Advice> {
        if self.record.setup.settings.info_mode == mode {
            return self.advice();
        }
        let mut record = self.record.clone();
        record.setup.settings.info_mode = mode;
        Self {
            course: self.course.clone(),
            record,
            rules: self.rules.clone(),
            clock_s: self.clock_s,
            attempt: self.attempt,
            ball_lie: self.ball_lie,
            last_lie: self.last_lie,
            rest_lie: self.rest_lie,
            original_lie: self.original_lie,
            trace_limit: 0,
            traces: VecDeque::new(),
        }
        .advice()
    }

    pub(crate) fn last_stroke(&self) -> Option<(&StrokeFacts, StrokeResult)> {
        // The record keeps the resolved stroke and its rest. Rebuild its report
        // from the lie and clock the stroke originally read.
        let mut strokes = self.stroke_facts();
        let last = strokes.next()?;
        let clock_s = strokes
            .next()
            .map_or(self.record.setup.tee_time_s, |previous| previous.clock_s);
        let world = self.world_at(clock_s);
        let club = last.club_id;
        let lie = world.address_lie(self.last_lie, &last.address.footprint(club), last.aim);
        let seed = self
            .record
            .setup
            .seed
            .derive(&format!("attempt/{}", last.attempt));
        let input = StrokeInput {
            delivery: DeliveryInput {
                club,
                address: last.address.clone(),
                lie,
                attempt_seed: seed,
            },
            aim: last.aim,
            gust_time_s: last.gust_time_s,
            flagstick_in: last.flagstick_in,
        };
        Some((
            last,
            stroke::play(&input, last.execution, &world, &mut NoProbe),
        ))
    }

    pub(crate) fn world_at(&self, clock_s: Real) -> World<'_> {
        self.record
            .setup
            .world(&self.course, clock_s)
            .expect("checked course")
    }
}
