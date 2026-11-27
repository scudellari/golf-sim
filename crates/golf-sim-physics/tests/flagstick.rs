use golf_sim_base::{Real, Surface, Vec3, trace::NoProbe};
use golf_sim_physics::{
    cup::{self, PuttFlagstick},
    integrator::{self, Options},
    plane::{ConstantAir, PlaneGround},
    samplers::{AirSample, GroundSample},
    tunables as t,
    values::{Cup, EndReason, EventKind, Launch, LaunchReadout, units},
};
use golf_sim_validate::Corpus;

#[test]
fn flagstick_putts_match_at_stimp_eight() {
    let corpus = Corpus::load_default(None).unwrap();
    let bilder = corpus
        .files()
        .iter()
        .find(|file| file.source == "BILDER")
        .unwrap();
    assert_eq!(bilder.targets.len(), 5);
    let conditions = &bilder.conditions;
    let air = ConstantAir::new(AirSample {
        temperature_c: conditions.air_temp_c,
        pressure_pa: conditions.pressure_pa,
        relative_humidity: conditions.humidity,
        ..AirSample::default()
    });
    // The source leaves green speed open. Accept at stimp 8 and report 10.
    // At 12 a nine-foot runout reaches the rim below the flag-out capture limit.
    for stimp_ft in [8.0, 10.0] {
        let ground = PlaneGround::new(
            GroundSample {
                surface: Surface::Green,
                firmness: conditions.green.firmness,
                moisture: conditions.green.moisture,
                green_speed_m: units::feet(stimp_ft),
                ..GroundSample::default()
            },
            Vec3::ZERO,
        );
        for cell in &bilder.targets {
            let line = cell.inputs["line"].as_str().unwrap();
            let flagstick = cell.inputs["flagstick"].as_str().unwrap();
            let (offset, stick) = PuttFlagstick::trial_cell(line, flagstick).unwrap();
            let past_m = units::feet(cell.inputs["past_ft"].as_number().unwrap());
            let measured = cell.measured["holed_fraction"].as_number().unwrap();
            let actual = cup::replay_putts(&ground, &air, past_m, offset, stick).unwrap();
            println!(
                "Stimp {stimp_ft:2.0} {line:10} flag {flagstick:8} {:5.1}% (measured {:5.1}%)",
                100.0 * actual,
                100.0 * measured,
            );
            if stimp_ft == 8.0 {
                assert!(
                    (actual - measured).abs() <= 0.15,
                    "{} predicted {actual:.3}, measured {measured:.3}",
                    cell.id
                );
            }
        }
    }
}

#[test]
fn fast_centred_putt_rebounds_from_the_flagstick() {
    let ground = PlaneGround::new(
        GroundSample {
            surface: Surface::Green,
            green_speed_m: units::feet(8.0),
            ..GroundSample::default()
        },
        Vec3::ZERO,
    );
    let cup = Cup {
        position: Vec3::ZERO,
        flagstick_in: true,
    };
    let putt = |speed: Real| {
        let launch = Launch::from_readout(
            &t::BALL,
            Vec3::new(0.0, 0.0, t::CUP_RADIUS_M + 0.001),
            Vec3::new(0.0, 0.0, -1.0),
            LaunchReadout {
                ball_speed_mps: speed,
                spin_rate_rad_s: -speed / t::BALL.radius_m,
                ..LaunchReadout::default()
            },
        );
        integrator::run(
            &launch,
            &t::BALL,
            &ConstantAir::new(AirSample::default()),
            &ground,
            Some(cup),
            Options::default(),
            &mut NoProbe,
        )
    };
    assert_eq!(putt(1.9).end_reason, EndReason::Holed);
    let shot = putt(2.002);
    let entry = shot.first_event(EventKind::CupEntry).unwrap();
    let rebound = shot.first_event(EventKind::Obstacle).unwrap();
    let rim_speed = entry.velocity.length();
    println!(
        "Centred flag-in putt at {rim_speed:.4} m/s at the rim: {:?}",
        shot.end_reason
    );
    assert!(rim_speed > 2.0);
    assert!(rebound.velocity.z > 0.0);
    assert_eq!(shot.end_reason, EndReason::Rest);
    assert!(shot.rest_position.z > t::CUP_RADIUS_M);
}
