//! Relief areas and rulings over supplied condition outlines.

use std::f64::consts::{PI, TAU};

use golf_sim_base::{
    PromptKind, Real, Vec2, Vec3, fm,
    geometry::{get_closest_point_to_segment, is_point_in_polygon, segment_intersects_segment},
    num::lerpf,
    trace::{Phase, Probe, Source},
};
use golf_sim_course::{
    course::PenaltyColor,
    legal::{Area, LegalMap},
};
use golf_sim_stroke::results::DropResult;

use crate::{
    rulings::{Consequence, HoleRules, RulesRefusal, RulesStep, Ruling, Status},
    transitions::{SPOT_TOLERANCE_M, penalty_options, start_hole, stroke_and_distance},
};

pub use crate::rulings::{ReliefArea, ReliefOption};

const CLEARANCE_M: Real = 0.002;
const SEARCH_STEP_M: Real = 0.10;
const SEARCH_DIRECTIONS: usize = 72;
const SEARCH_REFINEMENTS: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbnormalKind {
    TemporaryWater,
    GroundUnderRepair,
    ImmovableObstruction,
}

impl AbnormalKind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::TemporaryWater => "Temporary water",
            Self::GroundUnderRepair => "Ground under repair",
            Self::ImmovableObstruction => "Immovable obstruction",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AbnormalCondition {
    pub kind: AbnormalKind,
    pub outline: Vec<Vec2>,
}

impl AbnormalCondition {
    pub fn new(kind: AbnormalKind, outline: Vec<Vec2>) -> Self {
        Self { kind, outline }
    }

    pub fn interferes(
        &self,
        point: Vec2,
        offsets: &[Vec2],
        radius_m: Real,
        line_to: Option<Vec2>,
    ) -> bool {
        self.touches(point, radius_m)
            || offsets
                .iter()
                .any(|&offset| self.touches(translated(point, offset), 0.0))
            || line_to.is_some_and(|end| {
                edges(&self.outline)
                    .any(|(a, b)| segment_intersects_segment(point, end, a, b).is_some())
            })
    }

    pub fn touches(&self, point: Vec2, radius_m: Real) -> bool {
        is_point_in_polygon(point, &self.outline)
            || closest_edge(point, &self.outline)
                .is_some_and(|edge| (point - edge).length_squared() <= radius_m * radius_m)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReliefReason {
    AbnormalCondition {
        kind: AbnormalKind,
        outside_bunker: bool,
    },
    EmbeddedBall,
    UnplayableBall,
    PenaltyArea {
        color: PenaltyColor,
    },
    StrokeAndDistance {
        status: Status,
    },
    ProvisionalBall,
}

impl ReliefReason {
    pub const fn rule(self) -> &'static str {
        match self {
            Self::AbnormalCondition {
                outside_bunker: true,
                ..
            } => "16.1c(2)",
            Self::AbnormalCondition { .. } => "16.1",
            Self::EmbeddedBall => "16.3",
            Self::UnplayableBall => "19",
            Self::PenaltyArea { .. } => "17.1",
            Self::StrokeAndDistance { .. } => "18.1",
            Self::ProvisionalBall => "18.3",
        }
    }

    pub const fn condition(self) -> &'static str {
        match self {
            Self::AbnormalCondition { kind, .. } => kind.name(),
            Self::EmbeddedBall => "Embedded ball",
            Self::UnplayableBall => "Unplayable ball",
            Self::PenaltyArea {
                color: PenaltyColor::Red,
            } => "Red penalty area",
            Self::PenaltyArea {
                color: PenaltyColor::Yellow,
            } => "Yellow penalty area",
            Self::StrokeAndDistance {
                status: Status::OutOfBounds,
            } => "Out of bounds",
            Self::StrokeAndDistance {
                status: Status::Lost,
            } => "Lost ball",
            Self::StrokeAndDistance {
                status: Status::InWater,
            } => "In water",
            Self::StrokeAndDistance { .. } => "Previous spot",
            Self::ProvisionalBall => "Provisional ball",
        }
    }

    fn record<P: Probe>(self, penalty_strokes: usize, probe: &mut P) {
        if P::ENABLED {
            probe.begin_phase(Phase::Ruling);
            probe.value(
                &format!("Rule {} – {}", self.rule(), self.condition()),
                penalty_strokes as Real,
                "strokes",
                Source::Computed,
            );
        }
    }
}

pub const fn flat(point: Vec3) -> Vec2 {
    Vec2::new(point.x, point.z)
}

fn translated(point: Vec2, offset: Vec2) -> Vec2 {
    Vec2::new(point.x + offset.x, point.y + offset.y)
}

fn at_height(point: Vec2, height: Real) -> Vec3 {
    Vec3::new(point.x, height, point.y)
}

fn edges(outline: &[Vec2]) -> impl Iterator<Item = (Vec2, Vec2)> {
    outline
        .last()
        .into_iter()
        .chain(outline)
        .copied()
        .zip(outline.iter().copied())
}

pub fn closest_edge(point: Vec2, outline: &[Vec2]) -> Option<Vec2> {
    edges(outline)
        .map(|(a, b)| get_closest_point_to_segment(point, a, b))
        .min_by(|a, b| {
            (point - *a)
                .length_squared()
                .total_cmp(&(point - *b).length_squared())
        })
}

impl ReliefArea {
    pub fn back(&self) -> Vec2 {
        let away = flat(self.reference) - flat(self.hole);
        if away.length_squared() > 0.0 {
            away.normalized()
        } else {
            Vec2::new(1.0, 0.0)
        }
    }

    pub const fn on_line(&self) -> bool {
        matches!(
            self.option,
            ReliefOption::BackOnLine
                | ReliefOption::OutsideBunker
                | ReliefOption::BunkerAbnormalOutside
        )
    }

    pub const fn penalty(&self) -> usize {
        match self.option {
            ReliefOption::OutsideBunker => 2,
            ReliefOption::TemporaryWater
            | ReliefOption::GroundUnderRepair
            | ReliefOption::ImmovableObstruction
            | ReliefOption::Embedded
            | ReliefOption::Provisional => 0,
            _ => 1,
        }
    }

    pub fn reach_m(&self) -> Real {
        self.club_length_m
            * if self.option == ReliefOption::Lateral {
                2.0
            } else {
                1.0
            }
    }

    /// Draws the reach arc and, where it cuts the area, the arc equidistant from the hole.
    pub fn outline(&self, points_per_half_turn: usize) -> Vec<Vec2> {
        if self.on_line() {
            return Vec::new();
        }
        let reach = self.reach_m();
        let reference = flat(self.reference);
        let hole = flat(self.hole);
        let away = self.back();
        let from_hole = reference.distance_to(hole);
        let edge = if reach >= 2.0 * from_hole {
            PI
        } else {
            fm::acos(-reach / (2.0 * from_hole))
        };
        let arc = |centre: Vec2, radius: Real, from: Real, to: Real| {
            let count = (fm::ceil(points_per_half_turn as Real * from.abs() / PI) as usize).max(2);
            (0..=count).map(move |k| {
                translated(
                    centre,
                    away.rotated(lerpf(from, to, k as Real / count as Real)) * radius,
                )
            })
        };
        let mut outline: Vec<_> = arc(reference, reach, -edge, edge).collect();
        if edge < PI {
            let near =
                fm::acos((1.0 - reach * reach / (2.0 * from_hole * from_hole)).clamp(-1.0, 1.0));
            outline.extend(arc(hole, from_hole, near, -near));
        }
        outline
    }

    pub fn accepts_first(&self, point: Vec3, legal: &LegalMap<'_>) -> bool {
        if !self.allows(point, legal) {
            return false;
        }
        let offset = flat(point) - flat(self.reference);
        if self.on_line() {
            let back = self.back();
            offset.dot(back) > 0.0 && offset.cross(back).abs() <= SPOT_TOLERANCE_M
        } else {
            offset.length() <= self.reach_m()
        }
    }

    pub fn accepts_rest(&self, point: Vec3, first: Vec3, legal: &LegalMap<'_>) -> bool {
        if (self.on_line() || self.option == ReliefOption::Lateral)
            && point.is_finite()
            && self.area_at(point, legal) != self.area_at(first, legal)
        {
            return false;
        }
        if self.on_line() {
            self.allows(point, legal)
                && flat(point).distance_to(flat(first)) <= self.club_length_m
                && flat(point).distance_to(flat(self.hole))
                    >= flat(first).distance_to(flat(self.hole))
        } else {
            self.accepts_first(point, legal)
        }
    }

    pub fn allows(&self, point: Vec3, legal: &LegalMap<'_>) -> bool {
        if !point.is_finite() || legal.out_of_bounds(point, 0.0) {
            return false;
        }
        let plan = flat(point);
        if !self.bunker.is_empty()
            && is_point_in_polygon(plan, &self.bunker)
                == matches!(
                    self.option,
                    ReliefOption::OutsideBunker | ReliefOption::BunkerAbnormalOutside
                )
        {
            return false;
        }
        if self.penalty_area.as_ref().is_some_and(|area| {
            is_point_in_polygon(plan, &area.outline)
                || closest_edge(plan, &area.outline)
                    .is_some_and(|edge| plan.distance_to(edge) <= self.ball_radius_m)
        }) {
            return false;
        }
        let area = self.area_at(point, legal);
        if self.required_area.is_some_and(|required| required != area)
            || (self.green_placement && !matches!(area, Area::General | Area::Green))
        {
            return false;
        }
        let hole = flat(self.hole);
        if self.condition.as_ref().is_some_and(|condition| {
            condition.interferes(
                plan,
                &self.footprint,
                self.ball_radius_m,
                self.green_placement.then_some(hole),
            )
        }) {
            return false;
        }
        plan.distance_to(hole) >= flat(self.reference).distance_to(hole)
    }

    fn area_at(&self, point: Vec3, legal: &LegalMap<'_>) -> Area {
        legal.area_at(point, &self.tee, &self.green)
    }
}

pub fn extent_limit(outline: &[Vec2], origin: Vec2, offset: Vec2) -> Real {
    outline
        .iter()
        .map(|&point| (point - offset).distance_to(origin))
        .fold(0.0, Real::max)
}

/// Searches the translated condition edges, then refines the first clear spot on each ray.
pub fn nearest(origin: Vec3, area: &ReliefArea, legal: &LegalMap<'_>) -> Option<Vec3> {
    let polygon = &area.condition.as_ref()?.outline;
    if polygon.is_empty() || !origin.is_finite() {
        return None;
    }
    let here = flat(origin);
    let clear = |point| area.allows(at_height(point, origin.y), legal);
    let mut best = None;
    let mut distance = Real::INFINITY;
    let mut extent: Real = 0.0;
    for (offset, radius) in std::iter::once((Vec2::ZERO, area.ball_radius_m))
        .chain(area.footprint.iter().map(|&offset| (offset, 0.0)))
    {
        extent = extent.max(extent_limit(polygon, here, offset));
        for (from, to) in edges(polygon) {
            let from = from - offset;
            let to = to - offset;
            let edge = to - from;
            let normal = Vec2::new(edge.y, -edge.x).normalized() * (radius + CLEARANCE_M);
            for shift in [normal * -1.0, normal] {
                let a = translated(from, shift);
                let b = translated(to, shift);
                for point in [get_closest_point_to_segment(here, a, b), a, b] {
                    let gap = here.distance_to(point);
                    if gap < distance && clear(point) {
                        best = Some(point);
                        distance = gap;
                    }
                }
            }
        }
    }

    // Area and footprint boundaries can cut across an otherwise clear edge.
    for i in 0..SEARCH_DIRECTIONS {
        let direction = Vec2::from_angle(TAU * i as Real / SEARCH_DIRECTIONS as Real);
        let along = |distance| translated(here, direction * distance);
        let mut low = 0.0;
        let mut high = SEARCH_STEP_M;
        while high < distance.min(extent + 2.0) {
            if clear(along(high)) {
                for _ in 0..SEARCH_REFINEMENTS {
                    let mid = (low + high) * 0.5;
                    if clear(along(mid)) {
                        high = mid;
                    } else {
                        low = mid;
                    }
                }
                best = Some(along(high));
                distance = high;
                break;
            }
            low = high;
            high += SEARCH_STEP_M;
        }
    }
    best.map(|point| at_height(point, origin.y))
}

fn playable(rules: &HoleRules) -> Option<&Ruling> {
    let prompt = rules.prompt();
    prompt.ruling.filter(|_| prompt.kind == PromptKind::Play)
}

pub fn relief_available(
    rules: &HoleRules,
    legal: &LegalMap<'_>,
    area: &ReliefArea,
    condition: &AbnormalCondition,
) -> bool {
    playable(rules).is_some_and(|last| interfering_ball(last, legal, area, condition).is_some())
}

fn interfering_ball(
    last: &Ruling,
    legal: &LegalMap<'_>,
    area: &ReliefArea,
    condition: &AbnormalCondition,
) -> Option<Vec3> {
    let ball = last.spot.filter(|_| last.provisional.is_none())?;
    if legal.penalty_at(ball, area.ball_radius_m).is_some() {
        return None;
    }
    condition
        .interferes(
            flat(ball),
            &area.footprint,
            area.ball_radius_m,
            (area.area_at(ball, legal) == Area::Green).then(|| flat(area.hole)),
        )
        .then_some(ball)
}

pub fn ask_relief(
    rules: &HoleRules,
    mut area: ReliefArea,
    legal: &LegalMap<'_>,
    probe: &mut impl Probe,
) -> RulesStep {
    let Some((last, condition, origin)) = playable(rules).and_then(|last| {
        let condition = area.condition.as_ref()?;
        let origin = interfering_ball(last, legal, &area, condition)?;
        Some((last, condition, origin))
    }) else {
        return RulesStep::refused(rules, RulesRefusal::NoConditionInterference);
    };
    let outside_bunker = area.option == ReliefOption::BunkerAbnormalOutside;
    let reason = ReliefReason::AbnormalCondition {
        kind: condition.kind,
        outside_bunker,
    };
    area.reference = origin;
    let bunker = legal.bunker_at(origin);
    let origin_area = area.area_at(origin, legal);
    if outside_bunker {
        let Some(bunker) = bunker else {
            return RulesStep::refused(rules, RulesRefusal::NotInBunker);
        };
        area.bunker = bunker.to_vec();
        area.condition = None;
    } else {
        area.green_placement = origin_area == Area::Green;
        if !area.green_placement {
            area.bunker = bunker.unwrap_or_default().to_vec();
            area.required_area = Some(if origin_area == Area::Bunker {
                Area::Bunker
            } else {
                Area::General
            });
        }
        let Some(spot) = nearest(origin, &area, legal) else {
            return RulesStep::refused(rules, RulesRefusal::NoCompleteRelief);
        };
        area.reference = spot;
    }
    let place_at = (!outside_bunker && area.green_placement).then_some(area.reference);
    apply(rules, begin(last, area, place_at), reason, probe)
}

pub fn unplayable_options(
    rules: &HoleRules,
    in_bunker: bool,
    in_penalty: bool,
) -> Vec<ReliefOption> {
    if in_penalty || playable(rules).is_none_or(|last| last.provisional.is_some()) {
        return Vec::new();
    }
    let mut options = vec![
        ReliefOption::StrokeAndDistance,
        ReliefOption::BackOnLine,
        ReliefOption::Lateral,
    ];
    if in_bunker {
        options.push(ReliefOption::OutsideBunker);
    }
    options
}

pub fn declare_unplayable(
    rules: &HoleRules,
    mut area: ReliefArea,
    legal: &LegalMap<'_>,
    from_area: Area,
    probe: &mut impl Probe,
) -> RulesStep {
    let Some(ball) = rules.ball() else {
        return RulesStep::refused(rules, RulesRefusal::UnplayableOptionUnavailable);
    };
    let bunker = legal.bunker_at(ball);
    let in_penalty = legal.penalty_at(ball, area.ball_radius_m).is_some();
    if !unplayable_options(rules, bunker.is_some(), in_penalty).contains(&area.option) {
        return RulesStep::refused(rules, RulesRefusal::UnplayableOptionUnavailable);
    }
    let last = rules.last().expect("unplayable relief requires a ruling");
    let next = if area.option == ReliefOption::StrokeAndDistance {
        stroke_and_distance(last, area, from_area)
    } else {
        area.reference = ball;
        area.bunker = bunker.unwrap_or_default().to_vec();
        begin(last, area, None)
    };
    apply(rules, next, ReliefReason::UnplayableBall, probe)
}

pub fn embedded_available(rules: &HoleRules, area: &ReliefArea, legal: &LegalMap<'_>) -> bool {
    playable(rules).is_some_and(|last| embedded_ball(last, area, legal).is_some())
}

fn embedded_ball(last: &Ruling, area: &ReliefArea, legal: &LegalMap<'_>) -> Option<Vec3> {
    last.spot
        .filter(|&ball| last.embedded && area.area_at(ball, legal) == Area::General)
}

pub fn embedded_relief(
    rules: &HoleRules,
    mut area: ReliefArea,
    legal: &LegalMap<'_>,
    probe: &mut impl Probe,
) -> RulesStep {
    let Some((last, ball)) =
        playable(rules).and_then(|last| embedded_ball(last, &area, legal).map(|ball| (last, ball)))
    else {
        return RulesStep::refused(rules, RulesRefusal::EmbeddedBallUnavailable);
    };
    area.reference = ball;
    let back = area.back() * area.ball_radius_m;
    area.reference += at_height(back, 0.0);
    area.required_area = Some(Area::General);
    apply(
        rules,
        begin(last, area, None),
        ReliefReason::EmbeddedBall,
        probe,
    )
}

pub fn after_drop(
    rules: &HoleRules,
    drop: &DropResult,
    legal: &LegalMap<'_>,
    probe: &mut impl Probe,
) -> RulesStep {
    let Some((last, area)) = pending(rules, PromptKind::Drop) else {
        return RulesStep::refused(rules, RulesRefusal::NoDropPending);
    };
    let Some(first) = drop.first.filter(|&point| area.accepts_first(point, legal)) else {
        return RulesStep::refused(rules, RulesRefusal::DropFirstOutsideReliefArea);
    };
    let next = if let Some(rest) = drop
        .rest
        .filter(|&rest| area.accepts_rest(rest, first, legal))
    {
        finish(last, area, rest)
    } else {
        let drops = last.drops + 1;
        let place_at = (drops == 2).then_some(first);
        Ruling {
            drops,
            ..begin(last, area.clone(), place_at)
        }
    };
    apply(rules, next, reason_for(last, area), probe)
}

pub fn place(rules: &HoleRules, spot: Vec3, probe: &mut impl Probe) -> RulesStep {
    let Some((last, area)) = pending(rules, PromptKind::Place)
        .filter(|(last, _)| spot.is_finite() && last.place_at == Some(spot))
    else {
        return RulesStep::refused(rules, RulesRefusal::WrongPlacementSpot);
    };
    apply(
        rules,
        finish(last, area, spot),
        reason_for(last, area),
        probe,
    )
}

pub fn choose_penalty(
    rules: &HoleRules,
    mut area: ReliefArea,
    from_area: Area,
    probe: &mut impl Probe,
) -> RulesStep {
    let Some(last) = rules
        .last()
        .filter(|_| penalty_options(rules).contains(&area.option))
    else {
        return RulesStep::refused(rules, RulesRefusal::PenaltyOptionUnavailable);
    };
    let Some(penalty) = &last.penalty_area else {
        return RulesStep::refused(rules, RulesRefusal::PenaltyOptionUnavailable);
    };
    let reason = ReliefReason::PenaltyArea {
        color: penalty.color,
    };
    let next = match area.option {
        ReliefOption::Cancel => last.play_original(),
        ReliefOption::StrokeAndDistance => stroke_and_distance(last, area, from_area),
        _ => {
            let entry = last
                .penalty_entry
                .filter(|p| p.is_finite())
                .map(flat)
                .filter(|&entry| {
                    closest_edge(entry, &penalty.outline)
                        .is_some_and(|edge| entry.distance_to(edge) <= SPOT_TOLERANCE_M)
                });
            let Some(edge) = entry.or_else(|| closest_edge(flat(last.rest), &penalty.outline))
            else {
                return RulesStep::refused(rules, RulesRefusal::PenaltyOptionUnavailable);
            };
            area.reference = at_height(edge, last.rest.y);
            area.penalty_area = Some(penalty.clone());
            begin(last, area, None)
        }
    };
    apply(rules, next, reason, probe)
}

fn pending(rules: &HoleRules, kind: PromptKind) -> Option<(&Ruling, &ReliefArea)> {
    let prompt = rules.prompt();
    let last = prompt.ruling.filter(|_| prompt.kind == kind)?;
    Some((last, last.relief.as_ref()?))
}

fn begin(last: &Ruling, area: ReliefArea, place_at: Option<Vec3>) -> Ruling {
    let consequence = if place_at.is_some() {
        Consequence::Place
    } else {
        Consequence::Drop
    };
    Ruling {
        relief: Some(area),
        place_at,
        ..last.with_consequence(consequence, last.penalty_strokes, None)
    }
}

fn finish(last: &Ruling, area: &ReliefArea, spot: Vec3) -> Ruling {
    if area.option == ReliefOption::Provisional {
        return last.with_provisional(start_hole(spot).rules, false, false);
    }
    Ruling {
        relief: Some(area.clone()),
        ..last.with_consequence(
            Consequence::Relief,
            last.penalty_strokes + area.penalty(),
            Some(spot),
        )
    }
}

fn reason_for(last: &Ruling, area: &ReliefArea) -> ReliefReason {
    // Stroke and distance and provisional rulings can enter drop or place through other transitions.
    last.reason.unwrap_or_else(|| {
        if area.option == ReliefOption::Provisional {
            ReliefReason::ProvisionalBall
        } else {
            ReliefReason::StrokeAndDistance {
                status: last.status,
            }
        }
    })
}

fn apply(
    rules: &HoleRules,
    mut ruling: Ruling,
    reason: ReliefReason,
    probe: &mut impl Probe,
) -> RulesStep {
    ruling.reason = Some(reason);
    reason.record(ruling.penalty_strokes, probe);
    RulesStep::applied(rules.replace_last(ruling))
}
