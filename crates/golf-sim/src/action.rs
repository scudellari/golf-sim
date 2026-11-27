//! Decisions submitted to a hole.

use golf_sim_base::{ClubId, Vec2, Vec3};
use golf_sim_stroke::address::Address;
use golf_sim_swing::hands::Profile;

pub use golf_sim_rules::rulings::ReliefOption;
pub use golf_sim_swing::hands::Swing;

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Stroke(StrokeChoice),
    GoForward,
    Relief {
        option: ReliefOption,
        /// Stance offsets in the horizontal world plane.
        footprint: Vec<Vec2>,
    },
    Drop(Vec3),
    Place(Vec3),
}

#[derive(Clone, Debug, PartialEq)]
pub struct StrokeChoice {
    pub club: ClubId,
    pub address: Address,
    pub flagstick_in: Option<bool>,
    pub swing: Swing,
    pub(crate) aim: Aim,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Aim {
    At(Vec3),
    Along(Vec3),
}

impl StrokeChoice {
    pub fn stock(club: ClubId, aim_point: Vec3) -> Self {
        Self {
            club,
            address: Address::stock(club),
            flagstick_in: None,
            swing: Swing::Skill(Profile::SCRATCH),
            aim: Aim::At(aim_point),
        }
    }

    pub fn with_swing(mut self, swing: Swing) -> Self {
        self.swing = swing;
        self
    }

    pub fn with_aim_direction(mut self, direction: Vec3) -> Self {
        self.aim = Aim::Along(direction);
        self
    }
}
