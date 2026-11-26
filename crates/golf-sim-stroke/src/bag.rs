//! The standard bag in playing order.

use golf_sim_base::ClubId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Group {
    Woods,
    Irons,
    Wedges,
    Putter,
}

impl Group {
    pub const ALL: [Self; 4] = [Self::Woods, Self::Irons, Self::Wedges, Self::Putter];

    pub fn label(self) -> &'static str {
        match self {
            Self::Woods => "Woods",
            Self::Irons => "Irons",
            Self::Wedges => "Wedges",
            Self::Putter => "Putter",
        }
    }
}

const LABELS: [(&str, &str); ClubId::ALL.len()] = [
    ("Driver", "Dr"),
    ("3 Wood", "3W"),
    ("Hybrid", "Hy"),
    ("4 Iron", "4i"),
    ("5 Iron", "5i"),
    ("6 Iron", "6i"),
    ("7 Iron", "7i"),
    ("8 Iron", "8i"),
    ("9 Iron", "9i"),
    ("Pitching Wedge", "PW"),
    ("Gap Wedge", "GW"),
    ("Sand Wedge", "SW"),
    ("Lob Wedge", "LW"),
    ("Putter", "Pt"),
];

pub const fn ids() -> &'static [ClubId] {
    ClubId::ALL
}

pub fn label(club: ClubId) -> &'static str {
    LABELS[club as usize].0
}

pub fn short_label(club: ClubId) -> &'static str {
    LABELS[club as usize].1
}

pub fn group(club: ClubId) -> Group {
    match club {
        ClubId::Driver | ClubId::Wood3 | ClubId::Hybrid => Group::Woods,
        ClubId::Iron4
        | ClubId::Iron5
        | ClubId::Iron6
        | ClubId::Iron7
        | ClubId::Iron8
        | ClubId::Iron9 => Group::Irons,
        ClubId::PitchingWedge | ClubId::GapWedge | ClubId::SandWedge | ClubId::LobWedge => {
            Group::Wedges
        }
        ClubId::Putter => Group::Putter,
    }
}

/// Moves along the bag, stopping at either end.
pub fn step(club: ClubId, steps: i32) -> ClubId {
    let index = (club as i32)
        .saturating_add(steps)
        .clamp(0, ids().len() as i32 - 1);
    ids()[index as usize]
}
