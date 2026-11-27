//! A hole's rulings and the decision they leave to the player.

use crate::relief::{AbnormalCondition, ReliefReason};

use golf_sim_base::{PromptKind, Real, Vec2, Vec3};
use golf_sim_course::{
    course::PenaltyColor,
    legal::{Area, PenaltyArea},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Holed,
    OutOfBounds,
    Lost,
    InWater,
    InPlay,
    PenaltyArea,
    Doubtful,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Consequence {
    Holed,
    AsItLies,
    MaybeLost,
    LastSpot,
    Drop,
    Place,
    Relief,
    Choose,
    ProvisionalPlay,
    PickedUp,
}

impl Consequence {
    fn prompt_kind(self) -> PromptKind {
        match self {
            Self::Holed | Self::PickedUp => PromptKind::Done,
            Self::AsItLies | Self::LastSpot | Self::Relief | Self::ProvisionalPlay => {
                PromptKind::Play
            }
            Self::MaybeLost => PromptKind::MaybeLost,
            Self::Drop => PromptKind::Drop,
            Self::Place => PromptKind::Place,
            Self::Choose => PromptKind::Choose,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReliefOption {
    Cancel,
    StrokeAndDistance,
    BackOnLine,
    Lateral,
    OutsideBunker,
    TemporaryWater,
    GroundUnderRepair,
    ImmovableObstruction,
    Embedded,
    BunkerAbnormalOutside,
    Provisional,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PenaltyAreaValue {
    pub outline: Vec<Vec2>,
    pub color: PenaltyColor,
}

impl From<PenaltyArea<'_>> for PenaltyAreaValue {
    fn from(area: PenaltyArea<'_>) -> Self {
        Self {
            outline: area.outline.to_vec(),
            color: area.color,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReliefArea {
    pub option: ReliefOption,
    pub reference: Vec3,
    pub hole: Vec3,
    pub club_length_m: Real,
    pub bunker: Vec<Vec2>,
    pub penalty_area: Option<PenaltyAreaValue>,
    pub tee: Vec<Vec2>,
    pub green: Vec<Vec2>,
    pub required_area: Option<Area>,
    pub green_placement: bool,
    pub condition: Option<AbnormalCondition>,
    pub footprint: Vec<Vec2>,
    pub ball_radius_m: Real,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ruling {
    pub status: Status,
    pub consequence: Consequence,
    pub penalty_strokes: usize,
    pub reason: Option<ReliefReason>,
    pub relief: Option<ReliefArea>,
    pub drops: usize,
    pub place_at: Option<Vec3>,
    pub penalty_area: Option<PenaltyAreaValue>,
    pub penalty_entry: Option<Vec3>,
    pub playable_in_penalty_area: bool,
    pub embedded: bool,
    pub attempt: Option<usize>,
    pub provisional: Option<Box<HoleRules>>,
    pub provisional_from_last: bool,
    pub from: Vec3,
    pub rest: Vec3,
    pub spot: Option<Vec3>,
}

impl Ruling {
    pub(crate) fn new(
        status: Status,
        consequence: Consequence,
        from: Vec3,
        rest: Vec3,
        spot: Option<Vec3>,
    ) -> Self {
        Self {
            status,
            consequence,
            penalty_strokes: 0,
            reason: None,
            relief: None,
            drops: 0,
            place_at: None,
            penalty_area: None,
            penalty_entry: None,
            playable_in_penalty_area: false,
            embedded: false,
            attempt: None,
            provisional: None,
            provisional_from_last: false,
            from,
            rest,
            spot,
        }
    }

    pub(crate) fn with_consequence(
        &self,
        consequence: Consequence,
        penalty_strokes: usize,
        spot: Option<Vec3>,
    ) -> Self {
        Self {
            penalty_strokes,
            penalty_area: self.penalty_area.clone(),
            penalty_entry: self.penalty_entry,
            playable_in_penalty_area: self.playable_in_penalty_area,
            provisional: self.provisional.clone(),
            attempt: self.attempt,
            ..Self::new(self.status, consequence, self.from, self.rest, spot)
        }
    }

    pub(crate) fn play_original(&self) -> Self {
        Self {
            provisional: None,
            ..self.with_consequence(Consequence::AsItLies, self.penalty_strokes, Some(self.rest))
        }
    }

    pub(crate) fn with_provisional(
        &self,
        branch: HoleRules,
        waiting: bool,
        from_last: bool,
    ) -> Self {
        let (consequence, spot) = if waiting {
            (Consequence::MaybeLost, None)
        } else {
            (Consequence::ProvisionalPlay, branch.ball())
        };
        Self {
            provisional: Some(Box::new(branch)),
            provisional_from_last: from_last,
            ..self.with_consequence(consequence, self.penalty_strokes, spot)
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HoleRules {
    pub tee: Vec3,
    pub rulings: Vec<Ruling>,
}

impl HoleRules {
    pub fn strokes(&self) -> usize {
        self.rulings.len() - usize::from(self.picked_up()) + self.penalties()
    }

    pub fn penalties(&self) -> usize {
        self.rulings
            .iter()
            .map(|ruling| ruling.penalty_strokes)
            .sum()
    }

    pub fn last(&self) -> Option<&Ruling> {
        self.rulings.last()
    }

    pub fn ball(&self) -> Option<Vec3> {
        self.last().map_or(Some(self.tee), |ruling| ruling.spot)
    }

    pub fn holed(&self) -> bool {
        self.last()
            .is_some_and(|r| r.consequence == Consequence::Holed)
    }

    pub fn picked_up(&self) -> bool {
        self.last()
            .is_some_and(|r| r.consequence == Consequence::PickedUp)
    }

    pub fn pending(&self) -> bool {
        matches!(
            self.prompt().kind,
            PromptKind::MaybeLost | PromptKind::Drop | PromptKind::Place | PromptKind::Choose
        )
    }

    pub fn prompt(&self) -> Prompt<'_> {
        let ruling = self.last();
        Prompt {
            kind: ruling.map_or(PromptKind::Play, |r| r.consequence.prompt_kind()),
            ruling,
        }
    }

    pub(crate) fn with_ruling(&self, ruling: Ruling) -> Self {
        let mut next = self.clone();
        next.rulings.push(ruling);
        next
    }

    pub(crate) fn replace_last(&self, ruling: Ruling) -> Self {
        let last = self.rulings.len() - 1;
        let mut rulings = self.rulings[..last].to_vec();
        rulings.push(ruling);
        Self {
            tee: self.tee,
            rulings,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Prompt<'a> {
    pub kind: PromptKind,
    pub ruling: Option<&'a Ruling>,
}

impl Prompt<'_> {
    pub fn from_last_spot(&self) -> bool {
        self.ruling
            .is_some_and(|r| r.consequence == Consequence::LastSpot || r.provisional_from_last)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RulesRefusal {
    StrokeWhileFinishedOrPending,
    WrongStrokeSpot { start: Vec3, ball: Vec3 },
    NoRulingPending,
    ProvisionalNotPlayed,
    ProvisionalUnavailable,
    HoleFinished,
    NoConditionInterference,
    NotInBunker,
    NoCompleteRelief,
    UnplayableOptionUnavailable,
    EmbeddedBallUnavailable,
    NoDropPending,
    DropFirstOutsideReliefArea,
    WrongPlacementSpot,
    PenaltyOptionUnavailable,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RulesStep {
    pub rules: HoleRules,
    pub refusal: Option<RulesRefusal>,
}

impl RulesStep {
    pub fn prompt(&self) -> Prompt<'_> {
        self.rules.prompt()
    }

    pub(crate) fn applied(rules: HoleRules) -> Self {
        Self {
            rules,
            refusal: None,
        }
    }

    pub(crate) fn refused(rules: &HoleRules, reason: RulesRefusal) -> Self {
        Self {
            rules: rules.clone(),
            refusal: Some(reason),
        }
    }
}
