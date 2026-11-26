//! Results for the rules, playback, and the golfer's preview.

use golf_sim_base::{Real, Surface, Vec2, Vec3};
use golf_sim_conditions::wear::{WearMark, WearMarkKind};
use golf_sim_physics::values::{EndReason, ShotRequest, ShotResult, Zone};

pub use crate::wrappers::StrikeQuality;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    AtRest,
    Holed,
    InWater,
    TimedOut,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StrokeOutcome {
    pub end: End,
    pub start: Vec3,
    pub rest: Vec3,
    pub rest_surface: Surface,
    pub ball_radius_m: Real,
    pub carry_m: Real,
    pub total_m: Real,
    pub embedded: bool,
    pub penalty_entry: Option<Vec3>,
    pub marks: Vec<WearMark>,
}

impl StrokeOutcome {
    pub(crate) fn from_shot(shot: &ShotResult, request: &ShotRequest) -> Self {
        let end = match shot.end_reason {
            EndReason::Rest => End::AtRest,
            EndReason::Holed => End::Holed,
            EndReason::Water => End::InWater,
            EndReason::Timeout | EndReason::InvalidRelease | EndReason::InvalidInput => {
                End::TimedOut
            }
        };
        let radius = request.ball.radius_m;
        let embedded = shot.embedded
            || (end == End::AtRest
                && shot.touchdowns.iter().any(|touch| {
                    let offset = touch.contact_point - shot.rest_position;
                    touch.depth_m >= radius * 0.25
                        && Vec2::new(offset.x, offset.z).length() < radius
                }));
        let mut marks = Vec::new();
        if let Some(launch) = shot.launch
            && let (Some(start), Some(end)) = (launch.turf_contact_start, launch.turf_contact_end)
            && launch.turf_contact_depth_m > 0.0
        {
            marks.push(WearMark {
                kind: WearMarkKind::Divot,
                start,
                end,
                depth_m: launch.turf_contact_depth_m,
                surface: request.lie.surface,
            });
        }
        marks.extend(
            shot.touchdowns
                .iter()
                .filter(|touch| touch.depth_m > 0.0)
                .map(|touch| WearMark {
                    kind: WearMarkKind::PitchMark,
                    start: touch.contact_point,
                    end: touch.contact_point,
                    depth_m: touch.depth_m,
                    surface: touch.surface,
                }),
        );
        Self {
            end,
            start: request.position,
            rest: shot.rest_position,
            rest_surface: shot.rest_surface,
            ball_radius_m: radius,
            carry_m: shot.carry_m(),
            total_m: shot.total_m(),
            embedded,
            penalty_entry: shot
                .last_zone_entry(Zone::PenaltyArea)
                .map(|entry| entry.position),
            marks,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StrokeResult {
    pub playback: ShotResult,
    pub outcome: StrokeOutcome,
    /// Timing fault after the soft curve. Positive is late, and 0.5 is the window edge.
    pub fault: Real,
    pub quality: StrikeQuality,
    pub fat: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropResult {
    pub first: Option<Vec3>,
    pub rest: Option<Vec3>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Aloft {
    Same,
    More,
    Less,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Preview {
    pub carry_m: Real,
    pub target_distance_m: Real,
    pub carry_to_target_m: Real,
    pub plays_like_m: Real,
    pub wind_aloft_mps: Vec3,
    pub wind_here_mps: Vec3,
    pub aloft: Aloft,
    pub roll_m: Real,
    pub putt_line: Vec<Vec3>,
    pub has_hint: bool,
    pub hint_carry_m: Real,
    pub hint_apex_m: Real,
    pub hint_offline_m: Real,
}

impl Default for Preview {
    fn default() -> Self {
        Self {
            carry_m: Real::NAN,
            target_distance_m: Real::NAN,
            carry_to_target_m: Real::NAN,
            plays_like_m: Real::NAN,
            wind_aloft_mps: Vec3::ZERO,
            wind_here_mps: Vec3::ZERO,
            aloft: Aloft::Same,
            roll_m: Real::NAN,
            putt_line: Vec::new(),
            has_hint: false,
            hint_carry_m: 0.0,
            hint_apex_m: 0.0,
            hint_offline_m: 0.0,
        }
    }
}
