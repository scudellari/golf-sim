//! Golfer, setup, and execution tunables for the stroke model.

use std::f64::consts::PI;

use golf_sim_base::{ClubId, Real, Surface, Vec2};
use golf_sim_physics::values::units::{INCH, MPH};

use crate::{address::Setting, bag::Group};

pub const VERSION: &str = "0.1.8";
pub const BALL_WIDTH_M: Real = 0.04267;

pub const PUTTER_FULL_SPEED_MPS: Real = 3.2;
pub const SPEED_AT_ZERO_POWER: Real = 0.2;
pub const OVERSWING_SPEED_GAIN: Real = 0.25;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClubTuning {
    pub speed_mph: Real,
    pub attack_deg: Real,
    pub loft_delta_deg: Real,
    pub plane_deg: Real,
    pub strike_up_mm: Real,
    pub difficulty: Real,
    pub ball_widths: Real,
    pub hands_deg: Real,
    pub tee_in: Real,
    pub stance_m: Real,
    pub shape_face: Real,
    pub lie_deg: Real,
}

pub fn club(id: ClubId) -> ClubTuning {
    match id {
        ClubId::Driver => ClubTuning {
            speed_mph: 105.0,
            attack_deg: 2.0,
            loft_delta_deg: 2.5,
            plane_deg: 48.0,
            strike_up_mm: 3.0,
            difficulty: 1.2,
            ball_widths: 3.0,
            hands_deg: 0.0,
            tee_in: 1.5,
            stance_m: 0.55,
            shape_face: 0.7,
            lie_deg: 58.0,
        },
        ClubId::Wood3 => ClubTuning {
            speed_mph: 99.42477876106194,
            attack_deg: -1.0,
            loft_delta_deg: -1.0,
            plane_deg: 52.0,
            strike_up_mm: 0.0,
            difficulty: 1.2,
            ball_widths: 2.0,
            hands_deg: 1.0,
            tee_in: 0.5,
            stance_m: 0.50,
            shape_face: 0.6,
            lie_deg: 58.5,
        },
        ClubId::Hybrid => ClubTuning {
            speed_mph: 96.5,
            attack_deg: -2.1,
            loft_delta_deg: -5.5,
            plane_deg: 55.0,
            strike_up_mm: 0.0,
            difficulty: 1.1,
            ball_widths: 1.5,
            hands_deg: 2.0,
            tee_in: 0.5,
            stance_m: 0.46,
            shape_face: 0.5,
            lie_deg: 59.0,
        },
        ClubId::Iron4 => ClubTuning {
            speed_mph: 92.5,
            attack_deg: -2.6,
            loft_delta_deg: -7.0,
            plane_deg: 57.0,
            strike_up_mm: -2.0,
            difficulty: 1.15,
            ball_widths: 1.0,
            hands_deg: 4.0,
            tee_in: 0.25,
            stance_m: 0.44,
            shape_face: 0.4,
            lie_deg: 61.0,
        },
        ClubId::Iron5 => ClubTuning {
            speed_mph: 90.0,
            attack_deg: -2.9,
            loft_delta_deg: -8.0,
            plane_deg: 58.0,
            strike_up_mm: -2.0,
            difficulty: 1.1,
            ball_widths: 1.0,
            hands_deg: 4.0,
            tee_in: 0.25,
            stance_m: 0.43,
            shape_face: 0.4,
            lie_deg: 61.5,
        },
        ClubId::Iron6 => ClubTuning {
            speed_mph: 87.5,
            attack_deg: -3.3,
            loft_delta_deg: -8.0,
            plane_deg: 59.0,
            strike_up_mm: -2.5,
            difficulty: 1.05,
            ball_widths: 0.5,
            hands_deg: 4.0,
            tee_in: 0.25,
            stance_m: 0.42,
            shape_face: 0.4,
            lie_deg: 62.0,
        },
        ClubId::Iron7 => ClubTuning {
            speed_mph: 85.0,
            attack_deg: -3.4,
            loft_delta_deg: -10.0,
            plane_deg: 60.0,
            strike_up_mm: -3.0,
            difficulty: 1.0,
            ball_widths: 0.5,
            hands_deg: 4.0,
            tee_in: 0.25,
            stance_m: 0.41,
            shape_face: 0.33,
            lie_deg: 62.5,
        },
        ClubId::Iron8 => ClubTuning {
            speed_mph: 82.5,
            attack_deg: -3.7,
            loft_delta_deg: -10.5,
            plane_deg: 61.0,
            strike_up_mm: -3.5,
            difficulty: 0.97,
            ball_widths: 0.0,
            hands_deg: 4.0,
            tee_in: 0.25,
            stance_m: 0.40,
            shape_face: 0.33,
            lie_deg: 63.0,
        },
        ClubId::Iron9 => ClubTuning {
            speed_mph: 80.5,
            attack_deg: -4.2,
            loft_delta_deg: -11.0,
            plane_deg: 62.0,
            strike_up_mm: -4.0,
            difficulty: 0.94,
            ball_widths: 0.0,
            hands_deg: 4.0,
            tee_in: 0.25,
            stance_m: 0.39,
            shape_face: 0.33,
            lie_deg: 63.5,
        },
        ClubId::PitchingWedge => ClubTuning {
            speed_mph: 78.0,
            attack_deg: -4.5,
            loft_delta_deg: -11.0,
            plane_deg: 63.0,
            strike_up_mm: -4.5,
            difficulty: 0.9,
            ball_widths: 0.0,
            hands_deg: 5.0,
            tee_in: 0.25,
            stance_m: 0.36,
            shape_face: 0.3,
            lie_deg: 64.0,
        },
        ClubId::GapWedge => ClubTuning {
            speed_mph: 74.0,
            attack_deg: -4.4,
            loft_delta_deg: -13.5,
            plane_deg: 63.0,
            strike_up_mm: -5.0,
            difficulty: 0.9,
            ball_widths: 0.0,
            hands_deg: 5.0,
            tee_in: 0.25,
            stance_m: 0.35,
            shape_face: 0.3,
            lie_deg: 64.0,
        },
        ClubId::SandWedge => ClubTuning {
            speed_mph: 70.5,
            attack_deg: -4.6,
            loft_delta_deg: -16.0,
            plane_deg: 64.0,
            strike_up_mm: -5.5,
            difficulty: 0.9,
            ball_widths: 0.0,
            hands_deg: 5.0,
            tee_in: 0.25,
            stance_m: 0.34,
            shape_face: 0.3,
            lie_deg: 64.0,
        },
        ClubId::LobWedge => ClubTuning {
            speed_mph: 67.0,
            attack_deg: -4.8,
            loft_delta_deg: -17.0,
            plane_deg: 64.0,
            strike_up_mm: -6.0,
            difficulty: 0.92,
            ball_widths: 0.0,
            hands_deg: 5.0,
            tee_in: 0.25,
            stance_m: 0.34,
            shape_face: 0.3,
            lie_deg: 64.0,
        },
        ClubId::Putter => ClubTuning {
            speed_mph: 0.0,
            attack_deg: 0.0,
            loft_delta_deg: 0.0,
            plane_deg: 72.0,
            strike_up_mm: 0.0,
            difficulty: 0.6,
            ball_widths: 1.0,
            hands_deg: 0.0,
            tee_in: 0.0,
            stance_m: 0.30,
            shape_face: 0.0,
            lie_deg: 70.0,
        },
    }
}

pub fn club_speed_mps(id: ClubId) -> Real {
    if id == ClubId::Putter {
        PUTTER_FULL_SPEED_MPS
    } else {
        club(id).speed_mph * MPH
    }
}

const DEG: Real = PI / 180.0;

pub struct SettingTuning {
    pub step: Real,
    pub big_step: i32,
    pub range: [Real; 2],
}

pub const SETTINGS: [SettingTuning; Setting::ALL.len()] = [
    SettingTuning {
        step: DEG,
        big_step: 3,
        range: [-15.0 * DEG, 15.0 * DEG],
    },
    SettingTuning {
        step: 0.5 * BALL_WIDTH_M,
        big_step: 4,
        range: [-4.0 * BALL_WIDTH_M, 5.0 * BALL_WIDTH_M],
    },
    SettingTuning {
        step: DEG,
        big_step: 3,
        range: [-10.0 * DEG, 20.0 * DEG],
    },
    SettingTuning {
        step: DEG,
        big_step: 3,
        range: [-5.0 * DEG, 15.0 * DEG],
    },
    SettingTuning {
        step: 0.5 * INCH,
        big_step: 2,
        range: [0.0, 3.0 * INCH],
    },
    SettingTuning {
        step: 0.05,
        big_step: 5,
        range: [0.25, 1.0],
    },
    SettingTuning {
        step: 0.25 * INCH,
        big_step: 2,
        range: [0.0, 2.5 * INCH],
    },
];

pub fn setting_range(group: Group, setting: Setting) -> [Real; 2] {
    match (group, setting) {
        (Group::Putter, Setting::Stance) => [-8.0 * DEG, 8.0 * DEG],
        (Group::Putter, Setting::BallPosition) => [-2.0 * BALL_WIDTH_M, 3.0 * BALL_WIDTH_M],
        (Group::Putter, Setting::Face) => [-5.0 * DEG, 5.0 * DEG],
        (Group::Putter, Setting::Hands) => [-5.0 * DEG, 8.0 * DEG],
        (Group::Putter, Setting::SwingLength) => [0.10, 1.0],
        (Group::Putter, Setting::TeeHeight) => [0.0, 0.0],
        _ => SETTINGS[setting as usize].range,
    }
}

pub const SHAPE_STANCE_RAD: Real = 3.0 * DEG;
pub const HEIGHT_BALL_M: Real = BALL_WIDTH_M;
pub const HEIGHT_HANDS_RAD: Real = -2.0 * DEG;
pub const INTENT_STEPS: i32 = 2;

pub const PUTTER_STANCE_AWAY_M: Real = 0.3;
pub const STANCE_REACH_FRACTION: Real = 0.9;
pub const STANCE_REACH_OFFSET_M: Real = 0.2;
pub const CLUB_ENTRY_ALONG_M: Real = -0.10;

pub const LOFT_PER_ARC: Real = 0.8;
pub const HANDS_LOW_POINT_M_PER_RAD: Real = 0.23;
pub const FACE_OPEN_LOFT: Real = 0.4;
pub const GRIP_SPEED_PER_M: Real = 0.5;
pub const GRIP_WINDOW_PER_M: Real = 4.0;
pub const TEE_ARC_FOLLOW: Real = 0.3;
pub const ARC_FOLLOWS_SETUP: Real = 0.8;
pub const LOW_POINT_LIMIT: Real = 0.6;
pub const UNTEED_ATTACK_MAX_DEG: Real = -1.0;
pub const HARD_GROUND_SOLE_CLEARANCE_M: Real = 0.001;

pub fn sole_clearance_m(surface: Surface) -> Real {
    match surface {
        Surface::Hardpan | Surface::CartPath => HARD_GROUND_SOLE_CLEARANCE_M,
        _ => 0.0,
    }
}

pub fn surface_difficulty(surface: Surface) -> Real {
    match surface {
        Surface::Tee => 0.9,
        Surface::Fairway | Surface::FirstCut | Surface::Fringe | Surface::Green => 1.0,
        Surface::Rough => 1.1,
        Surface::DeepRough => 1.2,
        Surface::Bunker => 1.25,
        Surface::Hardpan => 1.15,
        Surface::CartPath => 1.3,
        Surface::Water => 1.5,
    }
}

pub const SLOPE_ACROSS_DIFFICULTY_PER_RAD: Real = 4.0;
pub const SLOPE_ALONG_DIFFICULTY_PER_RAD: Real = 3.5;
pub const SHORT_SWING_DIFFICULTY: Real = 0.7;
pub const SLOPE_LIE_FRACTION: Real = 0.6;
pub const SLOPE_TOE_BIAS_M_PER_RAD: Real = -0.068;
pub const GRIP_TOE_RELIEF_M_PER_M: Real = 0.2;
pub const SLOPE_SPEED_LOSS_PER_RAD: Real = 0.25;
pub const SAND_FOOT_SPEED_LOSS: Real = 0.02;
pub const SLOPE_FAULT_BIAS_PER_RAD: Real = 2.5;
pub const FAULT_BIAS_LIMIT: Real = 0.3;

pub const BASE_WINDOW_S: Real = 0.040;
pub const MIN_WINDOW_S: Real = 0.016;
pub const METER_RATE_PER_S: Real = 1.0 / 1.1;
pub const RING_MIN_S: Real = 0.20;
pub const RING_MAX_S: Real = 0.42;
pub const TIMEOUT_AFTER_S: Real = 0.30;

#[derive(Clone, Copy)]
pub(crate) struct FaultPattern {
    pub path_deg: Real,
    pub face_deg: Real,
    pub loft_deg: Real,
    pub lp_m: Real,
    pub depth_m: Real,
    pub speed: Real,
    pub toe_m: Real,
}

// Late hangs back with an in-to-out path. Early comes over the top and steeper.
pub(crate) const LATE: FaultPattern = FaultPattern {
    path_deg: 3.5,
    face_deg: -1.0,
    loft_deg: 3.0,
    lp_m: -0.04,
    depth_m: 0.0,
    speed: -0.03,
    toe_m: 0.005,
};
pub(crate) const EARLY: FaultPattern = FaultPattern {
    path_deg: -5.0,
    face_deg: -1.0,
    loft_deg: -1.0,
    lp_m: 0.03,
    depth_m: -0.002,
    speed: -0.02,
    toe_m: -0.005,
};
pub(crate) const PUTT_LATE: FaultPattern = FaultPattern {
    path_deg: 0.0,
    face_deg: -1.5,
    loft_deg: 0.0,
    lp_m: 0.0,
    depth_m: 0.0,
    speed: 0.0,
    toe_m: 0.0,
};
pub(crate) const PUTT_EARLY: FaultPattern = FaultPattern {
    face_deg: 1.5,
    ..PUTT_LATE
};

pub(crate) struct Spread {
    pub face_deg: Real,
    pub path_deg: Real,
    pub speed: Real,
    pub toe_m: Real,
    pub lp_m: Real,
}

pub(crate) const SPREAD: Spread = Spread {
    face_deg: 1.2,
    path_deg: 1.2,
    speed: 0.02,
    toe_m: 0.004,
    lp_m: 0.015,
};
pub(crate) const PUTT_SPREAD: Spread = Spread {
    face_deg: 1.2,
    path_deg: 0.0,
    speed: 0.0,
    toe_m: 0.0,
    lp_m: 0.0,
};

pub(crate) struct Hidden {
    pub face_deg: Real,
    pub path_deg: Real,
    pub loft_deg: Real,
    pub speed: Real,
    pub lp_m: Real,
    pub depth_m: Real,
    pub toe_m: Real,
    pub up_m: Real,
}

pub(crate) const HIDDEN: Hidden = Hidden {
    face_deg: 0.12,
    path_deg: 0.20,
    loft_deg: 0.25,
    speed: 0.005,
    lp_m: 0.004,
    depth_m: 0.0004,
    toe_m: 0.0015,
    up_m: 0.0008,
};

pub const SPREAD_CAP_U: Real = 4.0;
pub const SPREAD_GAIN: Real = 3.0;
pub const PUTT_HIDDEN_SCALE: Real = 0.35;
pub const OVERSWING_NOISE_GAIN: Real = 10.0;
pub const OVERSHOOT_SPREAD_U: Real = 2.0;
pub const TIMEOUT_FAULT_MIN: Real = 1.2;
pub const TIMEOUT_FAULT_MAX: Real = 2.2;
pub const TIMEOUT_POWER_MIN: Real = 0.85;
pub const TIMEOUT_TOE_SD_M: Real = 0.012;

pub const DRIFT_START_SD_M: Vec2 = Vec2::new(0.007, 0.003);
pub const DRIFT_SLIDE_SD_MPS: Vec2 = Vec2::new(0.006, 0.002);
pub const DRIFT_WOBBLE_M: Vec2 = Vec2::new(0.0012, 0.0005);
pub const PUTT_DRIFT_SCALE: Real = 0.25;

pub const WIND_ALOFT_NOTICEABLE_MPS: Real = 2.0;

pub const PURE_RADIUS_M: Real = 0.006;
pub const PURE_FAULT: Real = 0.35;
pub const PURE_INTERFERENCE: Real = 0.1;
pub const MISHIT_RADIUS_M: Real = 0.018;
pub const FAT_ENTRY_M: Real = 0.025;
pub const FAT_DEPTH_M: Real = 0.0015;
pub const FAT_SPEED_LOSS: Real = 0.05;
