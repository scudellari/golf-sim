//! The current rules prompt and the decisions available to answer it.

use std::fmt;

use golf_sim_base::{ClubId, PromptKind, Refusal, Vec2, Vec3, log::tracing};
use golf_sim_course::legal::StanceFootprint;
pub use golf_sim_rules::rulings::Prompt;
use golf_sim_rules::{rulings::ReliefOption, transitions};

use super::{Hole, relief_ground};
use crate::{
    action::{Action, StrokeChoice},
    records::hole::Refusal as RecordRefusal,
};

#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum ActionRefusal {
    Rules(Refusal),
    Invalid(RecordRefusal),
    PreviewUnavailable,
}

impl From<RecordRefusal> for ActionRefusal {
    fn from(error: RecordRefusal) -> Self {
        Self::Invalid(error)
    }
}

impl fmt::Display for ActionRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rules(error) => error.fmt(f),
            Self::Invalid(error) => error.fmt(f),
            Self::PreviewUnavailable => f.write_str("preview is available in Caddie mode"),
        }
    }
}

impl std::error::Error for ActionRefusal {}

impl Hole {
    pub fn prompt(&self) -> Prompt<'_> {
        self.rules.prompt()
    }

    /// A playable stroke, available relief choices, and suggested drop or
    /// placement spots, as applicable. Stroke and relief inputs can be customized.
    pub fn legal(&self) -> Vec<Action> {
        let prompt = self.prompt();
        let mut actions = Vec::new();
        let world = self.world_at(self.clock_s);
        match prompt.kind {
            PromptKind::Play => {
                let club = self.previous_club().unwrap_or(ClubId::Iron7);
                let aim = self.first_aim(world.cup_position().expect("checked cup"));
                let choice =
                    StrokeChoice::stock(club, self.ball_lie.spot + aim).with_aim_direction(aim);
                let footprint = choice.address.footprint(club);
                actions.push(Action::Stroke(choice));
                if prompt.ruling.is_some() {
                    for option in [
                        ReliefOption::Cancel,
                        ReliefOption::StrokeAndDistance,
                        ReliefOption::BackOnLine,
                        ReliefOption::Lateral,
                        ReliefOption::OutsideBunker,
                        ReliefOption::Embedded,
                    ] {
                        let action = relief(option);
                        if self.accepts_decision(&action) {
                            actions.push(action);
                        }
                    }
                    let stance: Vec<_> = [
                        footprint.lead_foot_m,
                        footprint.trail_foot_m,
                        footprint.club_entry_m,
                    ]
                    .into_iter()
                    .map(|point| {
                        let offset = StanceFootprint::to_world(point, Vec3::ZERO, aim);
                        Vec2::new(offset.x, offset.z)
                    })
                    .collect();
                    for option in [
                        ReliefOption::TemporaryWater,
                        ReliefOption::GroundUnderRepair,
                        ReliefOption::ImmovableObstruction,
                        ReliefOption::BunkerAbnormalOutside,
                    ] {
                        let with_stance = Action::Relief {
                            option,
                            footprint: stance.clone(),
                        };
                        if self.accepts_decision(&with_stance) {
                            actions.push(with_stance);
                        } else {
                            let action = relief(option);
                            if self.accepts_decision(&action) {
                                actions.push(action);
                            }
                        }
                    }
                }
                return actions;
            }
            PromptKind::MaybeLost => {
                actions.push(Action::GoForward);
                if transitions::provisional_available(&self.rules) {
                    actions.push(relief(ReliefOption::Provisional));
                }
            }
            PromptKind::Choose => actions.extend(
                transitions::penalty_options(&self.rules)
                    .into_iter()
                    .map(relief),
            ),
            PromptKind::Drop => {
                if let Some(area) = prompt.ruling.and_then(|ruling| ruling.relief.as_ref()) {
                    actions.extend(
                        relief_ground::drop_spots(area, &world, &self.course)
                            .into_iter()
                            .map(Action::Drop),
                    );
                }
            }
            PromptKind::Place => {
                if let Some(spot) = prompt.ruling.and_then(|ruling| ruling.place_at) {
                    actions.push(Action::Place(spot));
                }
            }
            PromptKind::Done => {}
        }
        actions.retain(|action| self.accepts_decision(action));
        actions
    }

    pub(super) fn check_action_prompt(&self, action: &Action) -> Result<(), ActionRefusal> {
        let prompt = self.prompt().kind;
        let available = match action {
            Action::Stroke(_) => prompt == PromptKind::Play,
            Action::GoForward
            | Action::Relief {
                option: ReliefOption::Provisional,
                ..
            } => prompt == PromptKind::MaybeLost,
            Action::Relief { .. } => matches!(prompt, PromptKind::Play | PromptKind::Choose),
            Action::Drop(_) => prompt == PromptKind::Drop,
            Action::Place(_) => prompt == PromptKind::Place,
        };
        available
            .then_some(())
            .ok_or(ActionRefusal::Rules(Refusal::NotNow { prompt }))
    }

    fn accepts_decision(&self, action: &Action) -> bool {
        // A ruling can offer a drop whose area has no usable ground. Ask the
        // same decision path as act, including its ground checks.
        let mut trial = Self {
            course: self.course.clone(),
            record: self.record.clone(),
            rules: self.rules.clone(),
            clock_s: self.clock_s,
            attempt: self.attempt,
            ball_lie: self.ball_lie,
            last_lie: self.last_lie,
            rest_lie: self.rest_lie,
            original_lie: self.original_lie,
            trace_limit: 0,
            traces: Default::default(),
        };
        tracing::subscriber::with_default(tracing::subscriber::NoSubscriber::default(), || {
            trial.act(action.clone()).is_ok()
        })
    }
}

fn relief(option: ReliefOption) -> Action {
    Action::Relief {
        option,
        footprint: Vec::new(),
    }
}
