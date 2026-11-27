//! Pure rules transitions over the facts of a hole and its strokes.

use golf_sim_base::{PromptKind, Real, Vec2, Vec3};
use golf_sim_course::{
    course::PenaltyColor,
    legal::{Area, LegalMap},
};
use golf_sim_stroke::results::{End, StrokeOutcome};

use crate::rulings::{
    Consequence, HoleRules, ReliefArea, ReliefOption, RulesRefusal, RulesStep, Ruling, Status,
};

pub const SPOT_TOLERANCE_M: Real = 0.001;

pub fn start_hole(tee: Vec3) -> RulesStep {
    RulesStep::applied(HoleRules {
        tee,
        rulings: Vec::new(),
    })
}

pub fn status(outcome: &StrokeOutcome, legal: &LegalMap<'_>) -> Status {
    classify(outcome, legal).0
}

fn classify<'a>(
    outcome: &StrokeOutcome,
    legal: &LegalMap<'a>,
) -> (Status, Option<golf_sim_course::legal::PenaltyArea<'a>>) {
    let rest = outcome.rest;
    let radius = outcome.ball_radius_m;
    if outcome.end == End::Holed {
        (Status::Holed, None)
    } else if legal.out_of_bounds(rest, radius) {
        (Status::OutOfBounds, None)
    } else if let Some(area) = legal.penalty_at(rest, radius) {
        (Status::PenaltyArea, Some(area))
    } else if legal.in_lost_ball_zone(rest) {
        (Status::Lost, None)
    } else if outcome.end == End::InWater {
        (Status::InWater, None)
    } else {
        (Status::InPlay, None)
    }
}

pub const fn consequence(status: Status) -> Consequence {
    match status {
        Status::Holed => Consequence::Holed,
        Status::OutOfBounds | Status::Lost | Status::InWater | Status::Doubtful => {
            Consequence::MaybeLost
        }
        Status::InPlay => Consequence::AsItLies,
        Status::PenaltyArea => Consequence::Choose,
    }
}

pub fn after_stroke(
    rules: &HoleRules,
    outcome: &StrokeOutcome,
    legal: &LegalMap<'_>,
    hole: Option<Vec3>,
    attempt: Option<usize>,
    doubtful: bool,
) -> RulesStep {
    let Some(ball) = rules
        .ball()
        .filter(|_| rules.prompt().kind == PromptKind::Play)
    else {
        return RulesStep::refused(rules, RulesRefusal::StrokeWhileFinishedOrPending);
    };
    if (flat(outcome.start) - flat(ball)).length_squared() > SPOT_TOLERANCE_M * SPOT_TOLERANCE_M {
        return RulesStep::refused(
            rules,
            RulesRefusal::WrongStrokeSpot {
                start: outcome.start,
                ball,
            },
        );
    }
    if let Some(original) = rules.last()
        && let Some(branch) = &original.provisional
    {
        let played = after_stroke(branch, outcome, legal, hole, attempt, false);
        if let Some(reason) = played.refusal {
            return RulesStep::refused(rules, reason);
        }
        let holed = played.rules.holed();
        let nearer = hole.filter(|hole| hole.is_finite()).is_some_and(|hole| {
            (flat(outcome.rest) - flat(hole)).length_squared()
                < (flat(original.rest) - flat(hole)).length_squared()
        });
        let waiting = rules.replace_last(original.with_provisional(played.rules, true, false));
        return if nearer || holed {
            go_forward(&waiting, None, Area::General)
        } else {
            RulesStep::applied(waiting)
        };
    }
    let (mut found, penalty_area) = classify(outcome, legal);
    if doubtful && found == Status::InPlay {
        found = Status::Doubtful;
    }
    let next = consequence(found);
    let spot = (next == Consequence::AsItLies).then_some(outcome.rest);
    let mut ruling = Ruling::new(found, next, outcome.start, outcome.rest, spot);
    ruling.attempt = attempt;
    ruling.embedded = next == Consequence::AsItLies && outcome.embedded;
    if let Some(area) = penalty_area {
        ruling.penalty_area = Some(area.into());
        ruling.penalty_entry = outcome.penalty_entry;
        ruling.playable_in_penalty_area = outcome.end != End::InWater;
    }
    RulesStep::applied(rules.with_ruling(ruling))
}

pub fn go_forward(rules: &HoleRules, area: Option<ReliefArea>, from_area: Area) -> RulesStep {
    let prompt = rules.prompt();
    let Some(last) = prompt
        .ruling
        .filter(|_| prompt.kind == PromptKind::MaybeLost)
    else {
        return RulesStep::refused(rules, RulesRefusal::NoRulingPending);
    };
    if last.status == Status::Doubtful {
        return RulesStep::applied(rules.replace_last(last.play_original()));
    }
    if let Some(branch) = &last.provisional {
        if branch.last().is_none() {
            return RulesStep::refused(rules, RulesRefusal::ProvisionalNotPlayed);
        }
        let ruling = Ruling {
            provisional: None,
            ..last.with_consequence(
                Consequence::Relief,
                last.penalty_strokes + 1,
                Some(branch.tee),
            )
        };
        let mut adopted = rules.replace_last(ruling);
        adopted.rulings.extend(branch.rulings.iter().cloned());
        return RulesStep::applied(adopted);
    }
    if let Some(area) = area
        && !matches!(from_area, Area::Teeing | Area::Green)
    {
        return RulesStep::applied(rules.replace_last(stroke_and_distance(last, area, from_area)));
    }
    let ruling = last.with_consequence(
        Consequence::LastSpot,
        last.penalty_strokes + 1,
        Some(last.from),
    );
    RulesStep::applied(rules.replace_last(ruling))
}

pub fn provisional_available(rules: &HoleRules) -> bool {
    let prompt = rules.prompt();
    prompt.kind == PromptKind::MaybeLost
        && prompt.ruling.is_some_and(|last| {
            matches!(
                last.status,
                Status::OutOfBounds | Status::Lost | Status::Doubtful
            ) && last
                .provisional
                .as_ref()
                .is_none_or(|branch| branch.ball().is_some_and(Vec3::is_finite) && !branch.holed())
        })
}

pub fn declare_provisional(rules: &HoleRules, area: ReliefArea, from_area: Area) -> RulesStep {
    let Some(last) = rules
        .last()
        .filter(|_| provisional_available(rules) && area.option == ReliefOption::Provisional)
    else {
        return RulesStep::refused(rules, RulesRefusal::ProvisionalUnavailable);
    };
    let ruling = if let Some(branch) = &last.provisional {
        last.with_provisional((**branch).clone(), false, false)
    } else {
        stroke_and_distance(last, area, from_area)
    };
    RulesStep::applied(rules.replace_last(ruling))
}

pub fn penalty_options(rules: &HoleRules) -> Vec<ReliefOption> {
    let Some(last) = rules.last().filter(|last| {
        last.status == Status::PenaltyArea
            && matches!(
                last.consequence,
                Consequence::Choose | Consequence::AsItLies
            )
    }) else {
        return Vec::new();
    };
    let mut options = Vec::new();
    if last.playable_in_penalty_area {
        options.push(ReliefOption::Cancel);
    }
    options.extend([ReliefOption::StrokeAndDistance, ReliefOption::BackOnLine]);
    if last
        .penalty_area
        .as_ref()
        .is_some_and(|area| area.color == PenaltyColor::Red)
    {
        options.push(ReliefOption::Lateral);
    }
    options
}

/// End play at the current count. The round applies any score cap to its card.
pub fn pick_up(rules: &HoleRules) -> RulesStep {
    if rules.prompt().kind == PromptKind::Done {
        return RulesStep::refused(rules, RulesRefusal::HoleFinished);
    }
    // Keep the prior ruling for the card, even if a decision was pending.
    let (status, from, rest) = rules
        .last()
        .map_or((Status::InPlay, rules.tee, rules.tee), |last| {
            (last.status, last.from, last.rest)
        });
    RulesStep::applied(rules.with_ruling(Ruling::new(
        status,
        Consequence::PickedUp,
        from,
        rest,
        None,
    )))
}

fn flat(point: Vec3) -> Vec2 {
    Vec2::new(point.x, point.z)
}

pub(crate) fn stroke_and_distance(last: &Ruling, mut area: ReliefArea, from_area: Area) -> Ruling {
    area.reference = last.from;
    area.required_area = Some(from_area);
    match from_area {
        Area::Teeing if area.option == ReliefOption::Provisional => {
            last.with_provisional(start_hole(last.from).rules, false, true)
        }
        Area::Teeing => Ruling {
            relief: Some(area),
            ..last.with_consequence(
                Consequence::LastSpot,
                last.penalty_strokes + 1,
                Some(last.from),
            )
        },
        Area::Green => Ruling {
            place_at: Some(area.reference),
            relief: Some(area),
            ..last.with_consequence(Consequence::Place, last.penalty_strokes, None)
        },
        Area::General | Area::Bunker | Area::Penalty => Ruling {
            relief: Some(area),
            ..last.with_consequence(Consequence::Drop, last.penalty_strokes, None)
        },
    }
}
