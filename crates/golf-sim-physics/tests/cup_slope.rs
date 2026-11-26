use golf_sim_base::{Real, Surface, Vec3, fm, trace::NoProbe};
use golf_sim_physics::{
    integrator::{self, Options},
    plane::{ConstantAir, PlaneGround},
    samplers::{GroundSample, GroundSampler},
    tunables as t,
    values::{Cup, EndReason, EventKind, Launch, units},
};

fn capture_limit(grade: Real, uphill: Vec3) -> Real {
    const AIM: Vec3 = Vec3::new(0.0, 0.0, -1.0);
    let ground = PlaneGround::sloped(
        GroundSample {
            surface: Surface::Green,
            green_speed_m: units::feet(10.0),
            ..GroundSample::default()
        },
        uphill,
        grade,
    );
    let normal = ground.sample_ground(0.0, 0.0).normal;
    let start = -AIM * (t::CUP_RADIUS_M + 0.02);
    let position = Vec3::new(
        start.x,
        ground.height_at(start.x, start.z) + t::BALL.radius_m / normal.y,
        start.z,
    );
    let tangent = AIM.slide(normal).normalized();
    let putt = |speed| {
        let velocity = tangent * speed;
        let launch = Launch {
            position,
            velocity,
            spin: normal.cross(velocity) / t::BALL.radius_m,
            ..Launch::default()
        };
        let shot = integrator::run(
            &launch,
            &t::BALL,
            &ConstantAir::default(),
            &ground,
            Some(Cup::default()),
            Options {
                playback: false,
                ..Options::default()
            },
            &mut NoProbe,
        );
        let entry = shot.first_event(EventKind::CupEntry).unwrap();
        let pure_roll = shot.first_event(EventKind::PureRoll).unwrap();
        assert!(pure_roll.time_s < entry.time_s);
        let captured = shot.first_event(EventKind::LipOut).is_none();
        if captured {
            assert_eq!(shot.end_reason, EndReason::Holed);
        }
        (
            captured,
            Vec3::new(entry.velocity.x, 0.0, entry.velocity.z).length(),
        )
    };
    let (mut slow, mut fast) = (1.3, 2.0);
    assert!(putt(slow).0);
    assert!(!putt(fast).0);
    for _ in 0..32 {
        let speed = (slow + fast) * 0.5;
        if putt(speed).0 {
            slow = speed;
        } else {
            fast = speed;
        }
    }
    (putt(slow).1 + putt(fast).1) * 0.5
}

#[test]
fn rolling_cup_capture_limits_follow_penner() {
    let uphill = Vec3::new(0.0, 0.0, -1.0);
    let across = Vec3::new(1.0, 0.0, 0.0);
    let flat = capture_limit(0.0, uphill);
    assert!((flat - 1.626).abs() < 1e-8, "flat {flat:.9} m/s");
    let mut limits = (flat, flat);
    for degrees in 0..=5 {
        let grade = fm::tan(Real::from(degrees).to_radians());
        let up = capture_limit(grade, uphill);
        let down = capture_limit(grade, -uphill);
        let left = capture_limit(grade, across);
        let right = capture_limit(grade, -across);
        println!("{degrees}° uphill {up:.6}, downhill {down:.6}, across {left:.6}/{right:.6} m/s");
        assert!((left - right).abs() < 1e-8);
        assert!((left - flat).abs() < 0.001);
        if degrees > 0 {
            assert!(up >= limits.0 && down <= limits.1);
        }
        limits = (up, down);
    }
    let (up, down) = (limits.0 - flat, limits.1 - flat);
    println!("5° shifts from flat {up:+.6} uphill, {down:+.6} downhill m/s");
    assert!((up - 0.08).abs() <= 0.02, "uphill shift {up:+.6} m/s");
    assert!((down + 0.07).abs() <= 0.02, "downhill shift {down:+.6} m/s");
}
