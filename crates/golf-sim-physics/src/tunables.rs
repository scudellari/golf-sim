//! Coefficients and equipment for the physics model.

use crate::values::{BallSpec, ClubCategory, ClubSpec, ObstacleKind};
use golf_sim_base::{ClubId, Real, Surface, num::deg_to_rad};

pub const VERSION: &str = "0.9.0";

// Constants

pub const GRAVITY: Real = 9.80665;
pub const KELVIN: Real = 273.15;
pub const MIN_WIND_REFERENCE_RATIO: Real = 1.1;
pub const R_DRY: Real = 287.058;
pub const R_VAPOR: Real = 461.495;
pub const BUCK_A_PA: Real = 611.21;
pub const BUCK_B: Real = 18.678;
pub const BUCK_C: Real = 257.14;
pub const BUCK_D: Real = 234.5;
pub const SUTHERLAND_MU0: Real = 1.716e-5;
pub const SUTHERLAND_T0: Real = 273.15;
pub const SUTHERLAND_S: Real = 110.4;
pub const ISA_P0: Real = 101325.0;
pub const STANDARD_TEMPERATURE_C: Real = 15.0;
pub const STANDARD_HUMIDITY: Real = 0.5;
pub const ISA_T0: Real = 288.15;
pub const ISA_LAPSE: Real = 0.0065;
pub const ISA_EXPONENT: Real = 5.25588;

// Integration

pub const FLIGHT_DT: Real = 1.0 / 60.0;
pub const GROUND_DT: Real = 1.0 / 240.0;
pub const PUTTING_GROUND_DT: Real = 1.0 / 480.0;
pub const SAMPLE_INTERVAL_S: Real = 1.0 / 60.0;
pub const MAX_SHOT_TIME_S: Real = 60.0;
pub const CONTACT_SLOP_M: Real = 1e-4;
pub const MIN_HOP_HEIGHT_M: Real = 0.003;
pub const SUPPORTED_LIE_HEIGHT_M: Real = 0.004;
pub const SUPPORT_BLEND_HEIGHT_M: Real = 0.008;
pub const TERRAIN_SWEEP_M: Real = 0.04;
pub const TERRAIN_REFINE_STEPS: usize = 9;
pub const SOLID_SWEEP_ITERATIONS: usize = 24;
pub const MAX_SIMULTANEOUS_CONTACTS: usize = 16;

// Aerodynamics

// BEGIN FIT AERO
pub const CD_BASE: Real = 0.18034;
pub const CD_SPIN: Real = 0.58685;
pub const CD_SPIN2: Real = -0.44087;
pub const CD_CRISIS_RISE: Real = 0.22628;
pub const CL_SCALE: Real = 0.52525;
pub const CL_EXPONENT: Real = 0.46963;
pub const CD_SPIN_PEAK: Real = 0.66556;
// END FIT AERO
pub const CD_CRISIS_RE: Real = 80000.0;
pub const CD_CRISIS_WIDTH: Real = 30000.0;
pub const SPIN_DECAY_LAMBDA: Real = 2.0e-5;

// Impact

// BEGIN FIT IMPACT
pub const IMPACT_FITS: [(ClubId, [Real; 2]); 10] = [
    (ClubId::Driver, [0.87068, -0.35713]),
    (ClubId::Wood3, [0.83662, 0.01642]),
    (ClubId::Hybrid, [0.80295, 0.18170]),
    (ClubId::Iron4, [0.71846, 0.18955]),
    (ClubId::Iron5, [0.69520, 0.18572]),
    (ClubId::Iron6, [0.68646, 0.21070]),
    (ClubId::Iron7, [0.65687, 0.23671]),
    (ClubId::Iron8, [0.67359, 0.30054]),
    (ClubId::Iron9, [0.66891, 0.30821]),
    (ClubId::PitchingWedge, [0.76955, 0.25628]),
];
pub const DRIVER_MOI_HEEL_TOE: Real = 3293.8;
pub const DRIVER_TANGENTIAL_SPEED_SLOPE: Real = 0.03220;
pub const IRON_COR_SPEED_SLOPE_DELTA: Real = 0.00372;
// END FIT IMPACT
pub const COR_REFERENCE_SPEED: Real = 48.8;
pub const COR_SPEED_SLOPE: Real = 0.0029;
pub const COR_LOW_SPEED_MAX: Real = 0.90;
pub const COR_PER_DEGREE_C: Real = 0.003;
pub const COR_TEMPERATURE_REFERENCE_C: Real = 23.0;
pub const EDGE_COR_FACTOR: Real = 0.8;
pub const EDGE_COR_BLEND_M: Real = 0.003;
pub const DRIVER_SMASH_LIMIT: Real = 1.50;
pub const TANGENTIAL_SPEED_RANGE_MPS: Real = 9.0;
pub const INTERFERENCE_COR_LOSS: Real = 0.25;
pub const INTERFERENCE_FRICTION_LOSS: Real = 0.75;
pub const INTERFERENCE_TANGENTIAL_LOSS: Real = 1.0;

// Club path through turf, grass, and sand

pub const ARC_LOOKBACK_M: Real = 0.35;
pub const ARC_SAMPLES: usize = 70;
pub const CUT_REFERENCE_SPEED: Real = 40.0;
pub const CUT_STATIC_FRACTION: Real = 0.15;
pub const SOLE_BOUNCE_DEPTH_RELIEF: Real = 0.6;
pub const HARD_SOLE_FRICTION: Real = 0.25;
pub const HARD_SOLE_IMPULSE_DEPTH_M: Real = 0.003;
pub const TURF_BLOCK_LOOKBACK_M: Real = 0.06;
pub const TURF_FULL_BLOCK_DEPTH_M: Real = 0.010;
pub const SAND_FULL_BLOCK_DEPTH_M: Real = 0.007;
pub const GRASS_CUT_ENERGY: Real = 3.5e4;
pub const GRASS_ABOVE_HEAD_SHARE: Real = 0.2;
pub const GRASS_FACE_CLOSE_DEG_PER_J: Real = 0.1;
pub const GRASS_FACE_CLOSE_MAX_DEG: Real = 15.0;
pub const GRASS_CLOSE_BY_CATEGORY: [Real; 6] = [0.4, 0.4, 0.7, 1.0, 1.0, 0.0];
pub const GRASS_FULL_BLOCK_M: Real = 0.01;
pub const GRASS_FILM_START_M: Real = 0.020;
pub const GRASS_FILM_FULL_M: Real = 0.040;
pub const GRASS_FILM_FRICTION_DECAY: Real = 2.0;
pub const GRASS_COVER_BALL_RADII: Real = 3.0;
pub const GRASS_ESCAPE_START_DEG: Real = 16.0;
pub const GRASS_ESCAPE_FULL_DEG: Real = 48.0;
pub const GRASS_COVER_LOSS_SHALLOW: Real = 0.65;
pub const GRASS_COVER_LOSS_STEEP: Real = 0.08;
pub const SAND_TRANSFER_LENGTH_M: Real = 0.08;
pub const SAND_BLEND_AREA_M2: Real = 0.0012;
pub const SAND_LIFT_START_DEG: Real = 25.0;
pub const SAND_LIFT_FULL_DEG: Real = 48.0;
pub const SAND_OPEN_BOUNCE: Real = 0.5;
pub const SAND_BOUNCE_GAIN: Real = 1.0;
pub const GRASS_PATH_DEFAULT_DENSITY: Real = 0.6;
pub const GRASS_PATH_DEFAULT_HEIGHT: [(Surface, Real); 3] = [
    (Surface::FirstCut, 0.018),
    (Surface::Rough, 0.045),
    (Surface::DeepRough, 0.09),
];
pub const WEDGE_SOLE_BOUNCE_DEG: Real = 10.0;
pub const SAND_CUSHION_DECAY_M: Real = 0.38;
pub const SAND_DIRECT_ACCESS_SOFT_M: Real = 0.011;
pub const SAND_DIRECT_ACCESS_FIRM_M: Real = 0.008;
pub const SAND_BURIED_BLEND_M: Real = 0.008;
pub const SAND_DIG_CLEARANCE_M: Real = 0.004;
pub const SAND_EXTRACTION_DECAY_M: Real = 0.08;
pub const SAND_BURIED_SPIN_KEEP: Real = 0.45;
pub const SAND_FACE_WEIGHT: Real = 0.55;
pub const SAND_BOUNCE_LAUNCH_GAIN: Real = 0.12;
pub const SAND_OPEN_SPEED_LOSS: Real = 0.25;
pub const SAND_SPIN_KEEP: Real = 0.5;
pub const SAND_OPEN_SPIN_LOSS: Real = 0.5;
pub const SAND_CUT_SOFT_SCALE: Real = 0.8;
pub const SAND_CUT_FIRM_SCALE: Real = 1.2;
pub const SAND_CUT_LOW_LOFT_SCALE: Real = 4.7;
pub const SAND_TRANSFER_SOFT_SCALE: Real = 0.85;
pub const SAND_TRANSFER_FIRM_SCALE: Real = 1.0;

// Ground: bounce, roll, putting

pub const CRATER_ONSET_MPS: Real = 0.5;
pub const CRATER_FULL_LOAD_MPS: Real = 3.0;
pub const BOUNCE_CRATER_DEG: Real = 15.4;
pub const BOUNCE_CRATER_SPEED: Real = 18.6;
pub const BOUNCE_CRATER_INCIDENCE_DEG: Real = 44.4;
pub const BOUNCE_COR_A: Real = 0.510;
pub const BOUNCE_COR_B: Real = -0.0375;
pub const BOUNCE_COR_C: Real = 0.000903;
pub const BOUNCE_COR_KNEE_SPEED: Real = 20.0;
pub const BOUNCE_COR_HIGH_SPEED: Real = 0.120;
// BEGIN FIT BOUNCE
pub const BOUNCE_CRATER_SCALE: Real = 0.89942;
pub const BOUNCE_CRATER_MAX_DEG: Real = 26.84816;
pub const BOUNCE_TURF_FRICTION: Real = 0.31480;
pub const BOUNCE_TANGENTIAL_COR: Real = -0.03341;
pub const BOUNCE_GREEN_MAX_DEG: Real = 13.50592;
pub const BOUNCE_GREEN_COR_FLOOR: Real = 0.22676;
pub const BOUNCE_GREEN_TANGENTIAL_COR: Real = 0.08786;
// END FIT BOUNCE
pub const BOUNCE_GREEN_CRATER_SCALE: Real = 1.4;
pub const BOUNCE_GREEN_CAP_SOFTNESS: Real = 0.5;
pub const BOUNCE_CRATER_SOFT_SCALE: Real = 1.5;
pub const BOUNCE_CRATER_FIRM_SCALE: Real = 0.5;
pub const BOUNCE_COR_SOFT_SCALE: Real = 0.8;
pub const BOUNCE_COR_FIRM_SCALE: Real = 1.2;
pub const BOUNCE_COR_WET_LOSS: Real = 0.15;
pub const BOUNCE_SOFT_FIRMNESS_LOSS: Real = 0.55;
pub const BOUNCE_SOFT_MOISTURE_LOSS: Real = 1.15;
pub const STIMP_EXIT_SPEED: Real = 1.83;
pub const ROLL_STOP_SPEED: Real = 0.02;
pub const ROLL_MOISTURE_SLOWDOWN: Real = 0.5;
pub const ROLL_FIRMNESS_GAIN: Real = 0.45;
pub const MOISTURE_REFERENCE: Real = 0.3;
// BEGIN FIT RUNOUT
pub const FAIRWAY_STIMP_M: Real = 1.46776;
// END FIT RUNOUT
pub const PITCH_DEPTH_PER_MPS: Real = 5.9e-4;
pub const PITCH_MOISTURE_GAIN: Real = 1.0;
pub const CUP_RADIUS_M: Real = 0.053975;
pub const CUP_DEPTH_M: Real = 0.1016;
pub const CUP_CAPTURE_SPEED: Real = 1.626;
pub const CUP_CAPTURE_EXPONENT: Real = 1.02;
pub const CUP_LIP_MAX_DEFLECT_RAD: Real = 0.6;
pub const CUP_LIP_SPEED_KEEP: Real = 0.75;

// Obstacles

pub const CANOPY_DRAG_PER_M: Real = 0.45;
pub const CANOPY_SPIN_LOSS_PER_S: Real = 5.0;
pub const FLAGSTICK_RADIUS_M: Real = 0.0095;
pub const FLAGSTICK_HEIGHT_M: Real = 2.13;
// Fitted to the MyGolfSpy putts tabled by Bilder on a stimp 8 plane.
pub const FLAGSTICK_RESTITUTION: Real = 0.829;
pub const FLAGSTICK_STIFF_RESTITUTION: Real = 0.828;
pub const FLAGSTICK_TANGENTIAL_FRICTION: Real = 0.016;

// Equipment

pub const BALL: BallSpec = BallSpec {
    mass_kg: 0.04593,
    radius_m: 0.021335,
    inertia_factor: 0.38,
};
pub const _IN: Real = 0.0254;
pub const _G_CM2: Real = 1.0e-7;
pub const PUTTER_COR: Real = 0.90;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceProperties {
    pub cor: Real,
    pub crater: Real,
    pub hard: bool,
    pub friction: Real,
    pub stimp_m: Real,
    pub uses_green_speed: bool,
    pub landing_loss: Real,
    pub grass_drag: Real,
    pub skid_friction: Real,
    pub cut_energy: Real,
}

const FAIRWAY: SurfaceProperties = SurfaceProperties {
    cor: 1.0,
    crater: 1.0,
    hard: false,
    friction: BOUNCE_TURF_FRICTION,
    stimp_m: FAIRWAY_STIMP_M,
    uses_green_speed: false,
    landing_loss: 0.0,
    grass_drag: 0.0,
    skid_friction: 0.4,
    cut_energy: 1.0e6,
};

/// Indexed by `Surface`.
pub const SURFACES: [SurfaceProperties; Surface::ALL.len()] = [
    FAIRWAY, // Tee
    FAIRWAY,
    SurfaceProperties {
        cor: 0.85,
        crater: 1.2,
        friction: 0.5,
        stimp_m: FAIRWAY_STIMP_M * 0.6,
        landing_loss: 0.15,
        grass_drag: 0.3,
        skid_friction: 0.5,
        ..FAIRWAY
    },
    SurfaceProperties {
        cor: 0.6,
        crater: 1.6,
        friction: 0.6,
        stimp_m: FAIRWAY_STIMP_M / 3.0,
        landing_loss: 0.4,
        grass_drag: 1.0,
        skid_friction: 0.6,
        ..FAIRWAY
    },
    SurfaceProperties {
        cor: 0.45,
        crater: 2.0,
        friction: 0.7,
        stimp_m: FAIRWAY_STIMP_M * 0.2,
        landing_loss: 0.65,
        grass_drag: 2.0,
        skid_friction: 0.7,
        ..FAIRWAY
    },
    SurfaceProperties {
        cor: 0.95,
        friction: 0.43,
        stimp_m: 2.0,
        uses_green_speed: true,
        grass_drag: 0.05,
        ..FAIRWAY
    },
    SurfaceProperties {
        friction: 0.43,
        stimp_m: 3.05,
        uses_green_speed: true,
        skid_friction: 0.25,
        ..FAIRWAY
    },
    SurfaceProperties {
        cor: 0.25,
        crater: 2.5,
        friction: 0.6,
        stimp_m: FAIRWAY_STIMP_M * (4.0 / 15.0),
        landing_loss: 0.92,
        grass_drag: 1.5,
        skid_friction: 0.7,
        cut_energy: 4.0e5,
        ..FAIRWAY
    },
    SurfaceProperties {
        cor: 0.28,
        crater: 0.0,
        hard: true,
        friction: 0.45,
        stimp_m: 1.2,
        cut_energy: 0.0,
        ..FAIRWAY
    },
    SurfaceProperties {
        cor: 0.35,
        crater: 0.0,
        hard: true,
        friction: 0.5,
        stimp_m: 2.7,
        skid_friction: 0.5,
        cut_energy: 0.0,
        ..FAIRWAY
    },
    SurfaceProperties {
        cor: 0.0,
        friction: 1.0,
        stimp_m: 0.1,
        grass_drag: 10.0,
        skid_friction: 1.0,
        ..FAIRWAY
    },
];

pub const fn surface(kind: Surface) -> &'static SurfaceProperties {
    &SURFACES[kind as usize]
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObstacleProperties {
    pub cor: Real,
    pub friction: Real,
}

const fn obstacle_properties(cor: Real, friction: Real) -> ObstacleProperties {
    ObstacleProperties { cor, friction }
}

/// Indexed by `ObstacleKind`.
pub const OBSTACLES: [ObstacleProperties; 7] = [
    obstacle_properties(0.45, 0.4),  // Trunk
    obstacle_properties(0.3, 0.5),   // Branch
    obstacle_properties(0.6, 0.35),  // Rock
    obstacle_properties(0.55, 0.35), // Wall
    obstacle_properties(0.15, 0.7),  // Bunker face
    obstacle_properties(0.3, 0.3),   // Flagstick
    obstacle_properties(0.35, 0.45), // Cup rim
];

pub const fn obstacle(kind: ObstacleKind) -> &'static ObstacleProperties {
    &OBSTACLES[kind as usize]
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClubHead {
    pub cg_depth: Real,
    pub cg_up: Real,
    pub moi_v: Real,
    pub moi_ht: Real,
    pub moi_f: Real,
    pub bulge_in: Real,
    pub roll_in: Real,
    pub face_h: Real,
    pub face_c: Real,
    pub face_w: Real,
    pub mu: Real,
    pub e_t_toe: Option<Real>,
    pub arc: Real,
    pub sole_w: Real,
    pub falloff_toe: Real,
    pub falloff_up: Real,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClubEntry {
    pub category: ClubCategory,
    pub loft_deg: Real,
    pub length_in: Real,
    pub mass_g: Real,
    pub head: ClubHead,
}

pub const CLUBS: [(ClubId, ClubEntry); 3] = [
    (
        ClubId::Driver,
        ClubEntry {
            category: ClubCategory::Driver,
            loft_deg: 10.5,
            length_in: 45.5,
            mass_g: 200.0,
            head: ClubHead {
                cg_depth: 0.040,
                cg_up: -0.004,
                moi_v: 5100.0,
                moi_ht: DRIVER_MOI_HEEL_TOE,
                moi_f: 4000.0,
                bulge_in: 11.0,
                roll_in: 11.0,
                face_h: 0.058,
                face_c: 0.029,
                face_w: 0.050,
                mu: 0.40,
                e_t_toe: Some(0.0),
                arc: 1.9,
                sole_w: 0.09,
                falloff_toe: 60.0,
                falloff_up: 170.0,
            },
        },
    ),
    (
        ClubId::Wood3,
        ClubEntry {
            category: ClubCategory::Wood,
            loft_deg: 15.0,
            length_in: 43.0,
            mass_g: 212.0,
            head: ClubHead {
                cg_depth: 0.030,
                cg_up: -0.004,
                moi_v: 3500.0,
                moi_ht: 2000.0,
                moi_f: 2500.0,
                bulge_in: 12.0,
                roll_in: 11.0,
                face_h: 0.038,
                face_c: 0.019,
                face_w: 0.042,
                mu: 0.40,
                e_t_toe: None,
                arc: 1.8,
                sole_w: 0.08,
                falloff_toe: 50.0,
                falloff_up: 80.0,
            },
        },
    ),
    (
        ClubId::Hybrid,
        ClubEntry {
            category: ClubCategory::Hybrid,
            loft_deg: 21.0,
            length_in: 40.5,
            mass_g: 228.0,
            head: ClubHead {
                cg_depth: 0.022,
                cg_up: -0.003,
                moi_v: 3000.0,
                moi_ht: 1500.0,
                moi_f: 2000.0,
                bulge_in: 13.0,
                roll_in: 13.0,
                face_h: 0.035,
                face_c: 0.018,
                face_w: 0.040,
                mu: 0.40,
                e_t_toe: None,
                arc: 1.7,
                sole_w: 0.06,
                falloff_toe: 60.0,
                falloff_up: 90.0,
            },
        },
    ),
];

const IRON_HEAD: ClubHead = ClubHead {
    cg_depth: 0.006,
    cg_up: -0.002,
    moi_v: 2500.0,
    moi_ht: 800.0,
    moi_f: 2500.0,
    bulge_in: 0.0,
    roll_in: 0.0,
    face_h: 0.048,
    face_c: 0.021,
    face_w: 0.036,
    mu: 0.40,
    e_t_toe: None,
    arc: 1.6,
    sole_w: 0.06,
    falloff_toe: 120.0,
    falloff_up: 150.0,
};

const WEDGE_HEAD: ClubHead = ClubHead {
    sole_w: 0.07,
    ..IRON_HEAD
};

const PUTTER: ClubEntry = ClubEntry {
    category: ClubCategory::Putter,
    loft_deg: 3.0,
    length_in: 34.0,
    mass_g: 350.0,
    head: ClubHead {
        cg_depth: 0.020,
        cg_up: -0.002,
        moi_v: 4500.0,
        moi_ht: 2000.0,
        moi_f: 3000.0,
        bulge_in: 0.0,
        roll_in: 0.0,
        face_h: 0.025,
        face_c: 0.012,
        face_w: 0.050,
        mu: 0.30,
        e_t_toe: None,
        arc: 1.1,
        sole_w: 0.03,
        falloff_toe: 150.0,
        falloff_up: 150.0,
    },
};

pub const fn standard_ball() -> BallSpec {
    BALL
}

pub const fn standard_club_ids() -> &'static [ClubId] {
    ClubId::ALL
}

pub fn standard_club(id: ClubId) -> ClubSpec {
    let (loft_deg, length_in, mass_g) = match id {
        ClubId::Driver => return club_spec(id, CLUBS[0].1, [0.0, 0.0]),
        ClubId::Wood3 => return club_spec(id, CLUBS[1].1, [0.0, 0.0]),
        ClubId::Hybrid => return club_spec(id, CLUBS[2].1, [0.0, 0.0]),
        ClubId::Iron4 => (24.0, 38.5, 248.0),
        ClubId::Iron5 => (27.0, 38.0, 255.0),
        ClubId::Iron6 => (30.0, 37.5, 262.0),
        ClubId::Iron7 => (34.0, 37.0, 269.0),
        ClubId::Iron8 => (38.0, 36.5, 276.0),
        ClubId::Iron9 => (42.0, 36.0, 283.0),
        ClubId::PitchingWedge => (46.0, 35.75, 290.0),
        ClubId::GapWedge => (50.0, 35.5, 295.0),
        ClubId::SandWedge => (56.0, 35.25, 300.0),
        ClubId::LobWedge => (60.0, 35.0, 300.0),
        ClubId::Putter => return club_spec(id, PUTTER, [PUTTER_COR, 0.0]),
    };
    let (category, head, fallback) = if loft_deg >= 46.0 {
        (ClubCategory::Wedge, WEDGE_HEAD, [0.76, 0.26])
    } else {
        (ClubCategory::Iron, IRON_HEAD, [0.0, 0.0])
    };
    club_spec(
        id,
        ClubEntry {
            category,
            loft_deg,
            length_in,
            mass_g,
            head,
        },
        fallback,
    )
}

fn club_spec(id: ClubId, entry: ClubEntry, fallback: [Real; 2]) -> ClubSpec {
    let fit = IMPACT_FITS.iter().find(|(club, _)| *club == id);
    let [cor, restitution_up] = fit.map_or(fallback, |(_, fit)| *fit);
    let head = entry.head;
    ClubSpec {
        id: id.name().to_owned(),
        category: entry.category,
        loft_rad: deg_to_rad(entry.loft_deg),
        length_m: entry.length_in * _IN,
        head_mass_kg: entry.mass_g / 1000.0,
        cor,
        cor_speed_slope_delta: if entry.category == ClubCategory::Iron && fit.is_some() {
            IRON_COR_SPEED_SLOPE_DELTA
        } else {
            0.0
        },
        cor_falloff_toe: head.falloff_toe,
        cor_falloff_up: head.falloff_up,
        cg_depth_m: head.cg_depth,
        cg_up_m: head.cg_up,
        cg_toe_m: 0.0,
        moi_vertical: head.moi_v * _G_CM2,
        moi_heel_toe: head.moi_ht * _G_CM2,
        moi_forward: head.moi_f * _G_CM2,
        bulge_radius_m: head.bulge_in * _IN,
        roll_radius_m: head.roll_in * _IN,
        face_center_height_m: head.face_c,
        face_height_m: head.face_h,
        face_half_width_m: head.face_w,
        face_friction: head.mu,
        tangential_restitution_up: restitution_up,
        tangential_restitution_toe: head.e_t_toe.unwrap_or(restitution_up),
        tangential_restitution_speed_slope: if entry.category == ClubCategory::Driver {
            DRIVER_TANGENTIAL_SPEED_SLOPE
        } else {
            0.0
        },
        arc_radius_m: head.arc,
        sole_width_m: head.sole_w,
        sole_bounce_rad: if entry.category == ClubCategory::Wedge {
            deg_to_rad(WEDGE_SOLE_BOUNCE_DEG)
        } else {
            0.0
        },
    }
}
