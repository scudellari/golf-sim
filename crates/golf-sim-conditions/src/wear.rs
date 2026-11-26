//! Divots and ball marks left on the course.

use std::{collections::BTreeMap, f64::consts::TAU};

use golf_sim_base::{Real, Surface, Vec2, Vec3, fm, num::fposmod};

const CELL_M: Real = 2.0;
const OWN_DIVOT_WIDTH_M: Real = 0.10;
const OWN_DIVOT_MIN_LENGTH_M: Real = 0.05;
const OWN_DIVOT_MAX_LENGTH_M: Real = 0.30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum WearMarkKind {
    Divot,
    PitchMark,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WearMark {
    pub kind: WearMarkKind,
    pub start: Vec3,
    pub end: Vec3,
    pub depth_m: Real,
    pub surface: Surface,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DivotState {
    Fresh,
    Old,
    SandFilled,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Divot {
    pub hole: i64,
    pub position: Vec3,
    pub bearing_rad: Real,
    pub length_m: Real,
    pub width_m: Real,
    pub depth_m: Real,
    pub state: DivotState,
    pub own: bool,
}

impl Divot {
    fn contains(&self, spot: Vec3) -> bool {
        let offset = Vec2::new(spot.x - self.position.x, spot.z - self.position.z);
        let along = Vec2::new(fm::sin(self.bearing_rad), -fm::cos(self.bearing_rad));
        let a = offset.dot(along) / (self.length_m * 0.5);
        let b = offset.dot(Vec2::new(-along.y, along.x)) / (self.width_m * 0.5);
        a * a + b * b <= 1.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BallMark {
    pub hole: i64,
    pub position: Vec3,
    pub depth_m: Real,
    pub own: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Wear {
    divots: Vec<Divot>,
    ball_marks: Vec<BallMark>,
    cells: BTreeMap<(i64, i64), Vec<usize>>,
}

impl Wear {
    pub fn new(divots: Vec<Divot>, ball_marks: Vec<BallMark>) -> Self {
        let mut cells = BTreeMap::<_, Vec<_>>::new();
        for (i, divot) in divots.iter().enumerate() {
            cells.entry(cell(divot.position)).or_default().push(i);
        }
        Self {
            divots,
            ball_marks,
            cells,
        }
    }

    pub fn divots(&self) -> &[Divot] {
        &self.divots
    }

    pub fn ball_marks(&self) -> &[BallMark] {
        &self.ball_marks
    }

    pub fn with_marks(&self, marks: &[WearMark], hole: i64) -> Self {
        use Surface::{Fairway, FirstCut, Fringe, Green, Rough, Tee};

        let mut next = self.clone();
        for mark in marks {
            match (mark.kind, mark.surface) {
                (WearMarkKind::Divot, Tee | Fairway | FirstCut | Rough) => {
                    let cut = Vec2::new(mark.end.x - mark.start.x, mark.end.z - mark.start.z);
                    let divot = Divot {
                        hole,
                        position: (mark.start + mark.end) * 0.5,
                        bearing_rad: fposmod(fm::atan2(cut.x, -cut.y), TAU),
                        length_m: cut
                            .length()
                            .clamp(OWN_DIVOT_MIN_LENGTH_M, OWN_DIVOT_MAX_LENGTH_M),
                        width_m: OWN_DIVOT_WIDTH_M,
                        depth_m: mark.depth_m,
                        state: DivotState::Fresh,
                        own: true,
                    };
                    next.cells
                        .entry(cell(divot.position))
                        .or_default()
                        .push(next.divots.len());
                    next.divots.push(divot);
                }
                (WearMarkKind::PitchMark, Green | Fringe) => next.ball_marks.push(BallMark {
                    hole,
                    position: mark.start,
                    depth_m: mark.depth_m,
                    own: true,
                }),
                _ => {}
            }
        }
        next
    }

    pub fn divot_at(&self, spot: Vec3) -> Option<&Divot> {
        let (x, z) = cell(spot);
        // Overlapping divots are resolved in z, then x cell order, then insertion order.
        for z in z.saturating_sub(1)..=z.saturating_add(1) {
            for x in x.saturating_sub(1)..=x.saturating_add(1) {
                if let Some(indices) = self.cells.get(&(x, z)) {
                    for &index in indices {
                        let divot = &self.divots[index];
                        if divot.contains(spot) {
                            return Some(divot);
                        }
                    }
                }
            }
        }
        None
    }
}

fn cell(position: Vec3) -> (i64, i64) {
    (
        fm::floor(position.x / CELL_M) as i64,
        fm::floor(position.z / CELL_M) as i64,
    )
}
