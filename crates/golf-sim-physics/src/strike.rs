//! Clubhead contact with the ground and grass before it reaches the ball.

use std::f64::consts::FRAC_PI_2;

use golf_sim_base::{
    Real, Surface, Vec3, fm,
    num::{clampf, deg_to_rad, lerpf, rad_to_deg, smoothstep},
};

use crate::{
    samplers::{GroundSampler, is_long_grass},
    tunables as t,
    values::{Lie, ShotRequest, UP},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreImpact {
    pub club_speed_mps: Real,
    pub face_close_rad: Real,
    pub face_deloft_rad: Real,
    pub interference: Real,
    pub cushion: Real,
    pub sole_height_m: Real,
    pub low_point_ahead_m: Real,
    pub low_point_height_m: Real,
    pub entry_behind_m: Real,
    pub turf_start: Option<Vec3>,
    pub turf_end: Option<Vec3>,
    pub turf_depth_m: Real,
    pub sand_fraction: Real,
    pub sand_blend: Real,
    pub sand_transfer: Real,
    pub sand_bounce_rad: Real,
    pub sand_opening: Real,
    pub direct_sand_access: Real,
    pub sand_spin_transfer: Real,
    pub grass_transfer: Real,
    pub grass_film: Real,
}

impl Default for PreImpact {
    fn default() -> Self {
        Self {
            club_speed_mps: 0.0,
            face_close_rad: 0.0,
            face_deloft_rad: 0.0,
            interference: 0.0,
            cushion: 0.0,
            sole_height_m: 0.0,
            low_point_ahead_m: 0.0,
            low_point_height_m: 0.0,
            entry_behind_m: 0.0,
            turf_start: None,
            turf_end: None,
            turf_depth_m: 0.0,
            sand_fraction: 0.0,
            sand_blend: 0.0,
            sand_transfer: 1.0,
            sand_bounce_rad: 0.0,
            sand_opening: 0.0,
            direct_sand_access: 1.0,
            sand_spin_transfer: 1.0,
            grass_transfer: 1.0,
            grass_film: 0.0,
        }
    }
}

pub fn before_ball(
    request: &ShotRequest,
    forward: Vec3,
    right: Vec3,
    ground: &(impl GroundSampler + ?Sized),
) -> PreImpact {
    let club = &request.club;
    let d = &request.delivery;
    let lie = &request.lie;
    let position = request.position;
    let radius = club.arc_radius_m;
    let tan_attack = fm::tan(d.attack_rad);
    let sole_height = d.sole_height_m;
    let mut contact = PreImpact {
        low_point_ahead_m: -radius * tan_attack,
        low_point_height_m: sole_height - radius * tan_attack * tan_attack * 0.5,
        ..PreImpact::default()
    };
    let path = forward * fm::cos(d.path_rad) + right * fm::sin(d.path_rad);
    let base_height = ground.sample_ground(position.x, position.z).height_m;
    let bunker = lie.surface == Surface::Bunker;
    let long_grass = is_long_grass(lie.surface);
    let spin_loft_deg = rad_to_deg(d.dynamic_loft_rad - d.attack_rad);
    let sand_lift = smoothstep(t::SAND_LIFT_START_DEG, t::SAND_LIFT_FULL_DEG, spin_loft_deg);
    let mut sand_scale = lerpf(t::SAND_CUT_LOW_LOFT_SCALE, 1.0, sand_lift);
    if bunker {
        sand_scale *= lerpf(
            t::SAND_CUT_SOFT_SCALE,
            t::SAND_CUT_FIRM_SCALE,
            lie.sand_firmness,
        );
    }

    let dx = t::ARC_LOOKBACK_M / t::ARC_SAMPLES as Real;
    let mass = club.head_mass_kg;
    let mut energy = 0.5 * mass * d.club_speed_mps * d.club_speed_mps;
    let reference_energy = 0.5 * mass * t::CUT_REFERENCE_SPEED * t::CUT_REFERENCE_SPEED;
    let static_energy = reference_energy * t::CUT_STATIC_FRACTION / (1.0 - t::CUT_STATIC_FRACTION);
    let opening = (d.face_rad - d.path_rad).max(0.0);
    let sole_bounce = clampf(
        club.sole_bounce_rad + d.dynamic_loft_rad - club.loft_rad + opening * t::SAND_OPEN_BOUNCE,
        0.0,
        FRAC_PI_2,
    );
    let depth_keep = 1.0 - t::SOLE_BOUNCE_DEPTH_RELIEF * fm::sin(sole_bounce);
    let head_height = club.face_height_m * fm::cos(d.dynamic_loft_rad);
    let mut soil_area = 0.0;
    let mut grass_area = 0.0;
    let mut sand_area = 0.0;
    let mut previous_depth = 0.0;
    let mut previous_grass = 0.0;
    let mut sole_lift: Real = 0.0;
    let mut hard_loss: Real = 0.0;

    let samples = t::ARC_SAMPLES as i32;
    for i in (-samples..=samples).rev() {
        let x = dx * Real::from(i);
        let arc_y = sole_height - x * tan_attack + x * x / (2.0 * radius);
        let point_x = position.x - path.x * x;
        let point_z = position.z - path.z * x;
        let sample = ground.sample_strike_ground(point_x, point_z, lie.surface);
        let soil_y = sample.height_m - base_height;
        let hard = matches!(sample.surface, Surface::Hardpan | Surface::CartPath);
        if hard && i >= 0 {
            let overlap = (soil_y - arc_y).max(0.0);
            sole_lift = sole_lift.max(overlap);
            let approach = path + UP * (tan_attack - x / radius);
            let inward = (-approach.dot(sample.normal) / approach.length()).max(0.0);
            hard_loss = hard_loss.max(
                t::HARD_SOLE_FRICTION
                    * inward
                    * smoothstep(0.0, t::HARD_SOLE_IMPULSE_DEPTH_M, overlap),
            );
        }
        let club_y = arc_y + sole_lift;
        let depth = if hard {
            0.0
        } else {
            (soil_y - club_y).max(0.0)
        };
        if depth > 0.0 {
            let point = Vec3::new(point_x, sample.height_m, point_z);
            contact.turf_start.get_or_insert(point);
            contact.turf_end = Some(point);
            contact.turf_depth_m = contact.turf_depth_m.max(depth * depth_keep);
        }
        // The rest of the arc marks the divot. Only material before the ball costs energy.
        if i < 0 {
            continue;
        }

        let grass_height = if sample.grass_height_m >= 0.0 {
            sample.grass_height_m
        } else if sample.surface == lie.surface && lie.grass_height_m > 0.0 {
            lie.grass_height_m
        } else {
            t::GRASS_PATH_DEFAULT_HEIGHT
                .iter()
                .find(|(surface, _)| *surface == sample.surface)
                .map_or(0.0, |(_, height)| *height)
        };
        let density = if sample.grass_density >= 0.0 {
            sample.grass_density
        } else if sample.surface == lie.surface
            && (lie.grass_density > 0.0
                || (long_grass && lie.grass_height_m > 0.0 && lie.grass_density == 0.0))
        {
            lie.grass_density
        } else {
            t::GRASS_PATH_DEFAULT_DENSITY
        };
        let stand = clampf(
            soil_y + grass_height - club_y.max(soil_y),
            0.0,
            grass_height,
        );
        let grass = density
            * (stand.min(head_height) + t::GRASS_ABOVE_HEAD_SHARE * (stand - head_height).max(0.0));
        if i < samples {
            let slice = 0.5 * (depth + previous_depth) * dx;
            let sand = sample.surface == Surface::Bunker;
            if slice > 0.0 {
                soil_area += slice;
                let work = t::surface(sample.surface).cut_energy
                    * if sand { sand_scale } else { 1.0 }
                    * club.sole_width_m
                    * slice
                    * depth_keep;
                energy = ((energy + static_energy)
                    * fm::exp(-work / (reference_energy + static_energy))
                    - static_energy)
                    .max(0.0);
            }
            grass_area += 0.5 * (grass + previous_grass) * dx;
            if sand && x <= t::SAND_TRANSFER_LENGTH_M {
                sand_area += slice;
            }
        }
        if depth > 0.0 && contact.entry_behind_m == 0.0 {
            contact.entry_behind_m = x;
        }
        previous_depth = depth;
        previous_grass = grass;
    }
    contact.sole_height_m = sole_height + sole_lift;
    energy *= (1.0 - hard_loss) * (1.0 - hard_loss);
    let grass_work = t::GRASS_CUT_ENERGY * club.sole_width_m * grass_area;
    contact.club_speed_mps = fm::sqrt(2.0 * (energy - grass_work).max(0.0) / mass);
    contact.face_close_rad = deg_to_rad(t::GRASS_FACE_CLOSE_MAX_DEG).min(
        grass_work
            * deg_to_rad(t::GRASS_FACE_CLOSE_DEG_PER_J)
            * t::GRASS_CLOSE_BY_CATEGORY[club.category as usize],
    );
    // Drag on the sole acts below the clubhead's centre of gravity.
    if long_grass {
        let lever = club.face_center_height_m + club.cg_up_m;
        contact.face_deloft_rad = deg_to_rad(t::GRASS_DELOFT_MAX_DEG)
            .min(grass_work * t::GRASS_DELOFT_RATE * lever / club.moi_heel_toe);
    }
    let full_block = if bunker {
        t::SAND_FULL_BLOCK_DEPTH_M
    } else {
        t::TURF_FULL_BLOCK_DEPTH_M
    };
    let turf_block = smoothstep(
        0.0,
        full_block,
        (-contact.sole_height_m).max(soil_area / t::TURF_BLOCK_LOOKBACK_M) * depth_keep,
    );
    let grass_block = grass_contact(
        &mut contact,
        lie,
        request.ball.radius_m,
        d.dynamic_loft_rad,
        spin_loft_deg,
    );
    contact.interference = clampf(turf_block.max(grass_block), 0.0, 1.0);
    contact.cushion = if long_grass {
        turf_block.max(contact.grass_film)
    } else {
        turf_block
    };
    if bunker {
        sand_contact(
            &mut contact,
            request,
            sand_area,
            sand_lift,
            sole_bounce,
            depth_keep,
            opening,
        );
    }
    contact
}

fn grass_contact(
    contact: &mut PreImpact,
    lie: &Lie,
    radius: Real,
    loft: Real,
    spin_loft_deg: Real,
) -> Real {
    if lie.grass_height_m <= 0.0 {
        return 0.0;
    }
    let contact_height = lie.ball_height_m + radius * (1.0 - fm::sin(loft));
    let block = lie.grass_density
        * clampf(
            (lie.grass_height_m - contact_height) / t::GRASS_FULL_BLOCK_M,
            0.0,
            1.0,
        );
    let cover_depth = t::GRASS_COVER_BALL_RADII * radius;
    let above_crown = lie.grass_height_m - (lie.ball_height_m + 2.0 * radius);
    let cover = lie.grass_density * smoothstep(0.0, cover_depth, above_crown);
    let climb = cover + lie.grass_density * (above_crown - cover_depth).max(0.0) / cover_depth;
    let escape = smoothstep(
        t::GRASS_ESCAPE_START_DEG,
        t::GRASS_ESCAPE_FULL_DEG,
        spin_loft_deg,
    );
    contact.grass_transfer = fm::exp(
        -climb
            * lerpf(
                t::GRASS_COVER_LOSS_SHALLOW,
                t::GRASS_COVER_LOSS_STEEP,
                escape,
            ),
    );
    let film_block = if is_long_grass(lie.surface) {
        block
    } else {
        block * (1.0 - cover)
    };
    contact.grass_film = film_block
        * smoothstep(
            t::GRASS_FILM_START_M,
            t::GRASS_FILM_FULL_M,
            lie.grass_height_m,
        );
    block
}

fn sand_contact(
    contact: &mut PreImpact,
    request: &ShotRequest,
    area: Real,
    loft_weight: Real,
    bounce: Real,
    depth_keep: Real,
    opening: Real,
) {
    let lie = &request.lie;
    let lift = 1.0 + fm::sin(bounce) * t::SAND_BOUNCE_GAIN;
    let depth = (-contact.sole_height_m).max(0.0);
    let burial = (-lie.ball_height_m).max(0.0);
    let access_depth = lerpf(
        t::SAND_DIRECT_ACCESS_SOFT_M,
        t::SAND_DIRECT_ACCESS_FIRM_M,
        lie.sand_firmness,
    );
    let cover = burial / access_depth;
    contact.direct_sand_access = fm::exp(-cover * cover);

    let digging_depth = (depth
        + (-fm::tan(request.delivery.attack_rad)).max(0.0) * request.ball.radius_m)
        * lerpf(1.0, depth_keep, lie.sand_firmness);
    let digging = smoothstep(0.0, burial + t::SAND_DIG_CLEARANCE_M, digging_depth);
    let buried = smoothstep(0.0, t::SAND_BURIED_BLEND_M, burial);
    let displaced = smoothstep(0.0, t::SAND_BLEND_AREA_M2, area * lift);
    contact.sand_fraction = lerpf(displaced, digging * displaced.max(digging), buried);
    contact.sand_blend = loft_weight * lerpf(1.0, contact.sand_fraction, buried);
    contact.interference = lerpf(
        contact.interference,
        1.0,
        contact.sand_fraction * loft_weight,
    );
    contact.sand_bounce_rad = bounce;
    contact.sand_opening = fm::sin(opening.min(FRAC_PI_2));
    let sand_keep = lerpf(
        t::SAND_TRANSFER_SOFT_SCALE,
        t::SAND_TRANSFER_FIRM_SCALE,
        lie.sand_firmness,
    );
    contact.sand_transfer = lerpf(1.0, sand_keep, contact.sand_fraction)
        * fm::exp(-depth / t::SAND_CUSHION_DECAY_M - burial / t::SAND_EXTRACTION_DECAY_M);
    contact.sand_spin_transfer = lerpf(1.0, t::SAND_BURIED_SPIN_KEEP, buried);
}
