use golf_sim_base::{ClubId, Real, Surface, Vec3, trace::NoProbe};
use golf_sim_physics::{
    bounce, cup, flight, impact,
    plane::PlaneGround,
    roll,
    samplers::{AirSample, GroundSample},
    tunables::{BALL, standard_club},
    values::{Delivery, Lie, ShotRequest},
};

fn bits(value: Vec3) -> [u64; 3] {
    [value.x, value.y, value.z].map(Real::to_bits)
}

// Recorded cases evaluated with the Rust 0.11.0 model.
#[test]
fn impact_matches_parent_bits() {
    // physics/impact: lab-setup/test-course/000054.
    let request = ShotRequest::new(
        standard_club(ClubId::Iron7),
        BALL,
        Delivery {
            club_speed_mps: 37.55136,
            attack_rad: -0.06981317007977318,
            dynamic_loft_rad: 0.4188790204786391,
            ..Delivery::default()
        },
        Lie {
            ball_height_m: 0.005,
            ..Lie::default()
        },
    );
    let air = AirSample {
        temperature_c: 25.0,
        ..AirSample::default()
    };
    let ground = PlaneGround::new(GroundSample::default(), Vec3::ZERO);
    let launch = impact::strike(&request, &air, &ground, &mut NoProbe);
    assert!(launch.valid);
    assert_eq!(
        bits(launch.velocity),
        [0, 0x402d_cfc1_800f_0a24, 0xc047_f003_c307_a865],
    );
    assert_eq!(bits(launch.spin), [0x4086_91f7_1481_9cfa, 0, 0]);
}

#[test]
fn bounce_matches_parent_bits() {
    // physics/contact: case/bunker/000008.
    let ground = GroundSample {
        surface: Surface::Bunker,
        firmness: 0.3,
        moisture: 0.4,
        ..GroundSample::default()
    };
    let bounce = bounce::impact(
        Vec3::new(0.49773406982421875, -4.921878337860107, -4.379322528839111),
        Vec3::new(75.22418975830078, -7.01008415222168, 0.9852015376091003),
        &ground,
        &BALL,
        &mut NoProbe,
    );
    assert_eq!(
        bits(bounce.velocity),
        [
            0x3fa8_605d_ae48_c7a9,
            0x3fc6_e778_6b9d_c005,
            0xbfd6_f468_5a84_b738
        ],
    );
    assert_eq!(
        bits(bounce.spin),
        [
            0xc030_b568_54b8_a0e0,
            0xbffb_2c76_fe77_d5db,
            0xc005_8e2c_0501_f06e
        ],
    );
}

#[test]
fn roll_drag_matches_parent_bits() {
    // physics/contact: case/physics/001520.
    let air = roll::Air {
        density: 1.142277803197989,
        viscosity: 1.7892976260350732e-5,
        half_area_per_mass: 0.015567136230183501,
    };
    let drag = roll::air_drag(
        Vec3::new(2.3484232425689697, 0.8211579322814941, -21.703153610229492),
        Vec3::new(150.0, 0.0, 0.0),
        &air,
        &BALL,
    );
    assert_eq!(drag.to_bits(), 0x400b_b532_d2e5_1bc9);
}

#[test]
fn cup_capture_matches_parent_bits() {
    // physics/contact: case/physics/002607.
    assert_eq!(
        cup::putt_capture_speed(0.042).to_bits(),
        0x3fe4_25fb_4d44_05d3
    );
}

#[test]
fn current_coefficients_stay_within_lyu_regional_ranges() {
    // C68 scored these fits with both wedge launches held out. The permitted
    // absolute carry errors were 3.132 yd (short) and 1.066 yd (long).
    // Form                               Loss   Short    Long  Failed checks
    // Parent                            21.111  -3.132  -1.065   0
    // Capped power                      13.633  -4.015  -1.708  49
    // Patent                            18.404  -0.962  +3.192  62
    // Power bend, fixed drag crisis     14.958  -3.929  -0.499  46
    // Power bend, refit drag crisis     15.150  -2.972  +0.167  40
    // Parent lift bend, refit crisis    15.317  -4.118  -0.787  51
    // Power bend, patent drag           19.802  -0.648  +2.857  64
    // Patent lift bend, drag crisis     10.183  -3.751  -0.723  56
    // Patent lift bend, patent drag     17.437  -1.067  +3.425  63
    // The power bend with a refit drag crisis met the wedge launches and sampled Lyu
    // bounds. One carry gain error grew from 0.900 to 2.033 yd.
    // Patent drag exceeded the sampled Cd bounds by 0.170 to 0.218.
    //
    // Lyu et al. (2018), doi:10.3390/proceedings2060238, Figures 3a and 4a
    // (CC BY 4.0).
    // Figure 3a shows ball-to-ball extrema at sampled Reynolds numbers.
    // The limits here are the rounded extrema across each speed regime.
    // At Re near 75,000 the current zero-spin drag is about 0.218,
    // below the local minimum of about 0.225.
    for (start, end, lower, upper) in [
        (50_000, 100_000, 0.18, 0.53),
        (100_000, 250_000, 0.15, 0.24),
    ] {
        for reynolds in (start..=end).step_by(1_000) {
            let (drag, _) = flight::coefficients(Real::from(reynolds), 0.0);
            assert!(
                (lower..=upper).contains(&drag),
                "Cd {drag} at Re {reynolds} left {lower}..={upper}"
            );
        }
    }

    // Figure 4a pools the lift data above 32 m/s by spin ratio. Use
    // the rounded ranges of the plotted clusters nearest these ratios.
    for (ratio, lower, upper) in [
        (0.045, 0.08, 0.13),
        (0.08, 0.11, 0.18),
        (0.10, 0.13, 0.20),
        (0.18, 0.23, 0.28),
        (0.20, 0.24, 0.28),
        (0.31, 0.29, 0.34),
    ] {
        let (_, lift) = flight::coefficients(150_000.0, ratio);
        assert!(
            (lower..=upper).contains(&lift),
            "Cl {lift} at spin ratio {ratio} left {lower}..={upper}"
        );
    }
}
