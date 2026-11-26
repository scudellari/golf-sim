use golf_sim_base::{Real, Vec2, Vec3, fm, geometry::get_closest_point_to_segment, num::lerpf};

use crate::{
    course::{Course, CourseData, LegalKind, PenaltyColor},
    zones::{contains, plan, polygon_edges, surface_outlines},
};

const UP: Vec3 = Vec3::new(0.0, 1.0, 0.0);

const PAST_EDGE_M: Real = 1e-6;
const CHORD_PROBES: usize = 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Area {
    General,
    Teeing,
    Green,
    Bunker,
    Penalty,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PenaltyArea<'a> {
    pub outline: &'a [Vec2],
    pub color: PenaltyColor,
}

impl PenaltyArea<'_> {
    pub fn edge_near(&self, point: Vec2) -> Option<Vec2> {
        let mut best = None;
        let mut gap = Real::INFINITY;
        for (a, b) in polygon_edges(self.outline) {
            let near = get_closest_point_to_segment(point, a, b);
            let distance = (point - near).length_squared();
            if distance < gap {
                gap = distance;
                best = Some(near);
            }
        }
        best
    }
}

#[derive(Debug)]
pub struct LegalMap<'a> {
    in_bounds: Vec<[Vec2; 4]>,
    out_of_bounds: Vec<&'a [Vec2]>,
    lost_ball: Vec<&'a [Vec2]>,
    penalty: Vec<PenaltyArea<'a>>,
    bunkers: Vec<&'a [Vec2]>,
}

impl Course {
    pub fn legal(&self) -> LegalMap<'_> {
        LegalMap::new(self.data())
    }
}

impl<'a> LegalMap<'a> {
    pub fn new(course: &'a CourseData) -> Self {
        let mut map = Self {
            in_bounds: surface_outlines(course).collect(),
            out_of_bounds: Vec::new(),
            lost_ball: Vec::new(),
            penalty: Vec::new(),
            bunkers: course
                .bunkers
                .iter()
                .map(|bunker| bunker.outline.as_slice())
                .collect(),
        };
        for region in &course.regions {
            match region.legal {
                LegalKind::OutOfBounds => map.out_of_bounds.push(&region.polygon),
                LegalKind::LostBall => map.lost_ball.push(&region.polygon),
                LegalKind::PenaltyArea => map.penalty.push(PenaltyArea {
                    outline: &region.polygon,
                    color: region.penalty_color,
                }),
                _ => {}
            }
        }
        map
    }

    pub fn penalty_at(&self, point: Vec3, radius_m: Real) -> Option<PenaltyArea<'a>> {
        if !point.is_finite() {
            return None;
        }
        let p = plan(point);
        if let Some(&area) = self.penalty.iter().find(|area| contains(area.outline, p)) {
            return Some(area);
        }

        let mut best = None;
        let mut gap = radius_m * radius_m;
        for &area in &self.penalty {
            if let Some(edge) = area.edge_near(p) {
                let distance = (p - edge).length_squared();
                if distance <= gap {
                    gap = distance;
                    best = Some(area);
                }
            }
        }
        best
    }

    pub fn bunker_at(&self, point: Vec3) -> Option<&'a [Vec2]> {
        self.bunkers
            .iter()
            .copied()
            .find(|outline| contains(outline, plan(point)))
    }

    pub fn area_at(&self, point: Vec3, tee: &[Vec2], green: &[Vec2]) -> Area {
        let p = plan(point);
        if self.penalty_at(point, 0.0).is_some() {
            Area::Penalty
        } else if contains(tee, p) {
            Area::Teeing
        } else if contains(green, p) {
            Area::Green
        } else if self.bunker_at(point).is_some() {
            Area::Bunker
        } else {
            Area::General
        }
    }

    /// A ball touching the course boundary is in bounds.
    pub fn out_of_bounds(&self, centre: Vec3, radius_m: Real) -> bool {
        let p = plan(centre);
        !self.in_bounds_at(p)
            && !self
                .boundaries()
                .any(|outline| self.reaches_in(p, radius_m, outline))
    }

    pub fn in_lost_ball_zone(&self, centre: Vec3) -> bool {
        self.lost_ball
            .iter()
            .any(|outline| contains(outline, plan(centre)))
    }

    fn boundaries(&self) -> impl Iterator<Item = &[Vec2]> {
        self.in_bounds
            .iter()
            .map(<[_; 4]>::as_slice)
            .chain(self.out_of_bounds.iter().copied())
    }

    fn in_bounds_at(&self, p: Vec2) -> bool {
        self.in_bounds.iter().any(|outline| contains(outline, p))
            && !self
                .out_of_bounds
                .iter()
                .any(|outline| contains(outline, p))
    }

    fn reaches_in(&self, p: Vec2, radius: Real, outline: &[Vec2]) -> bool {
        for (a, b) in polygon_edges(outline) {
            let edge = b - a;
            let length = edge.length();
            if length == 0.0 {
                continue;
            }
            let direction = edge / length;
            let along = (p - a).dot(direction);
            let foot = along.clamp(0.0, length);
            let gap_squared = (p - edge_point(a, direction, foot)).length_squared();
            if gap_squared > radius * radius {
                continue;
            }

            // Inspect both sides of the chord. An overlapping exclusion can cover its midpoint.
            let half = fm::sqrt(radius * radius - gap_squared);
            let from = (along - half).clamp(0.0, length);
            let to = (along + half).clamp(0.0, length);
            let side = Vec2::new(direction.y, -direction.x) * PAST_EDGE_M;
            for k in 0..CHORD_PROBES {
                let at = lerpf(from, to, k as Real / (CHORD_PROBES - 1) as Real);
                let on_edge = edge_point(a, direction, at);
                if self.in_bounds_at(on_edge - side)
                    || self.in_bounds_at(edge_point(on_edge, side, 1.0))
                {
                    return true;
                }
            }
        }
        false
    }
}

fn edge_point(start: Vec2, direction: Vec2, distance: Real) -> Vec2 {
    Vec2::new(
        start.x + direction.x * distance,
        start.y + direction.y * distance,
    )
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StanceFootprint {
    pub lead_foot_m: Vec2,
    pub trail_foot_m: Vec2,
    pub club_entry_m: Vec2,
}

impl Default for StanceFootprint {
    fn default() -> Self {
        Self {
            lead_foot_m: Vec2::new(0.2, -0.7),
            trail_foot_m: Vec2::new(-0.2, -0.7),
            club_entry_m: Vec2::new(-0.1, 0.0),
        }
    }
}

impl StanceFootprint {
    pub fn centre_m(&self) -> Vec2 {
        Vec2::new(
            (self.lead_foot_m.x + self.trail_foot_m.x) * 0.5,
            (self.lead_foot_m.y + self.trail_foot_m.y) * 0.5,
        )
    }

    /// Projects an aim-frame offset onto the horizontal plane through the ball.
    pub fn to_world(point_m: Vec2, ball: Vec3, aim: Vec3) -> Vec3 {
        let forward = Vec3::new(aim.x, 0.0, aim.z).normalized();
        ball + forward * point_m.x + forward.cross(UP) * point_m.y
    }
}
