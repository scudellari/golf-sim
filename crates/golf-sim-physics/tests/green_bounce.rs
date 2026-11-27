use golf_sim_base::{Real, Surface, Vec3, fm, trace::NoProbe};
use golf_sim_physics::{
    bounce::{self, GreenClass, Outcome},
    facade,
    plane::{ConstantAir, PlaneGround},
    samplers::GroundSample,
    tunables::BALL,
    values::{EventKind, Launch, LaunchReadout},
};
use golf_sim_validate::{Corpus, Kind};

fn green(firmness: Real) -> GroundSample {
    GroundSample {
        surface: Surface::Green,
        firmness,
        ..GroundSample::default()
    }
}

fn contact(speed: Real, degrees: Real, spin: Real, firmness: Real, class: GreenClass) -> Outcome {
    let angle = degrees.to_radians();
    bounce::impact_with_green_class(
        Vec3::new(speed * fm::cos(angle), -speed * fm::sin(angle), 0.0),
        Vec3::new(0.0, 0.0, spin),
        &green(firmness),
        &BALL,
        class,
        &mut NoProbe,
    )
}

#[test]
fn stiffer_r04_greens_rebound_faster_and_shallower_with_more_spin_change() {
    let corpus = Corpus::load_default(None).unwrap();
    let targets = &corpus
        .files()
        .iter()
        .find(|file| file.source == "R04")
        .unwrap()
        .targets;
    for club in ["R04.5iron.", "R04.sw."] {
        let mut sites = targets
            .iter()
            .filter(|row| {
                row.kind == Kind::TurfImpact
                    && row.id.starts_with(club)
                    && row.id.ends_with(".calibration")
            })
            .map(|row| {
                let number = |name: &str| row.inputs[name].as_number().unwrap();
                let stiffness = number("stiffness_mpa");
                let spin = number("spin_rad_s");
                let out = contact(
                    number("speed_mps"),
                    number("angle_deg"),
                    spin,
                    bounce::green_firmness(stiffness),
                    GreenClass::Slipping,
                );
                (
                    stiffness,
                    out.velocity.length(),
                    fm::atan2(out.velocity.y, out.velocity.x).to_degrees(),
                    (spin - out.spin.z).abs(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(sites.len(), 6);
        sites.sort_by(|left, right| left.0.total_cmp(&right.0));
        for pair in sites.windows(2) {
            let [soft, firm] = pair else { unreachable!() };
            assert!(
                firm.1 > soft.1 && firm.2 < soft.2 && firm.3 > soft.3,
                "{club} {pair:?}"
            );
        }
        println!("{club} (MPa, m/s, degrees, spin change rad/s) {sites:?}");
    }
}

#[test]
fn green_class_controls_whether_high_backspin_survives() {
    for (speed, angle, spin) in [(22.65, 43.5, 551.75), (26.45, 44.25, 661.08)] {
        let slipping = contact(speed, angle, spin, 0.8, GreenClass::Slipping)
            .spin
            .z;
        let rolling = contact(speed, angle, spin, 0.8, GreenClass::Rolling).spin.z;
        assert!(
            slipping > 0.0 && rolling < 0.0,
            "{speed} m/s: {slipping}, {rolling}"
        );
        println!("{speed} m/s, {spin} rad/s in, {slipping:.2} slipping and {rolling:.2} rolling");
    }
}

#[test]
fn a_stroke_uses_the_loaded_green_response() {
    let at = Vec3::new(0.0, 0.01, 0.0);
    let launch = Launch::from_readout(
        &BALL,
        at,
        Vec3::new(1.0, 0.0, 0.0),
        LaunchReadout {
            ball_speed_mps: 22.86,
            launch_angle_rad: -45.0_f64.to_radians(),
            spin_rate_rad_s: 628.31853,
            ..LaunchReadout::default()
        },
    );
    let ground = green(1.0);
    let shot = facade::simulate_launch(
        &launch,
        &BALL,
        0.0,
        &ConstantAir::default(),
        &PlaneGround::new(ground, Vec3::ZERO),
        None,
        false,
        &mut NoProbe,
    );
    let land = shot.first_event(EventKind::Landing).unwrap();
    let rebound = shot.first_event(EventKind::Bounce).unwrap();
    let expected = bounce::impact_with_green_class(
        land.velocity,
        land.spin,
        &ground,
        &BALL,
        GreenClass::Slipping,
        &mut NoProbe,
    );
    assert_eq!(rebound.velocity, expected.velocity);
    assert_eq!(rebound.spin, expected.spin);
    assert!(rebound.spin.z > 0.0);
}

#[test]
fn other_surfaces_match_the_parent_bits() {
    let normal = Vec3::new(0.01998003013432026, 0.9990015625953674, 0.03996006026864052);
    let spin = Vec3::new(-190.0, 0.0, 20.0);
    let slow = Vec3::new(2.200000047683716, -6.600000381469727, -22.0);
    let fast = Vec3::new(4.900000095367432, -14.700000762939453, -49.0);
    // Contact fixture inputs with v0.10.0 outputs.
    let cases = [
        (
            "001617",
            Surface::Fairway,
            0.8,
            0.2,
            slow,
            [1.2937844568935173, 7.555967763381535, -12.426134611745082],
            [-649.7914135429592, 21.232151537306756, -54.91559356503511],
        ),
        (
            "001680",
            Surface::Fairway,
            0.2,
            0.85,
            fast,
            [1.720997797295799, 10.91423075248505, -14.903060510239161],
            [-825.5458687819678, 25.501305508930393, -76.8921458126702],
        ),
        (
            "001830",
            Surface::Rough,
            0.8,
            0.2,
            fast,
            [1.9984020547923977, 12.250111179829144, -17.39168670689956],
            [-952.7780827462993, 29.431538439135938, -88.74267807037336],
        ),
        (
            "001867",
            Surface::Rough,
            0.2,
            0.85,
            slow,
            [0.45478284029897553, 2.934213917214369, -4.387528360003392],
            [-238.926651802341, 9.125680419990326, -18.512516442976594],
        ),
        (
            "002217",
            Surface::Bunker,
            0.8,
            0.2,
            slow,
            [0.31802321527209537, 1.9221063652077537, -3.0908695213011828],
            [-165.12538564841665, 6.306879066391124, -12.794246242139872],
        ),
        (
            "002280",
            Surface::Bunker,
            0.2,
            0.85,
            fast,
            [0.2757883850228812, 1.4656961793706917, -2.4460390346105156],
            [-128.38781711987826, 3.9659297827137507, -11.958166260485996],
        ),
    ];
    let bits = |v: Vec3| [v.x, v.y, v.z].map(Real::to_bits);
    for (id, surface, firmness, moisture, velocity, want_velocity, want_spin) in cases {
        let sample = GroundSample {
            surface,
            firmness,
            moisture,
            normal,
            ..GroundSample::default()
        };
        let out = bounce::impact(velocity, spin, &sample, &BALL, &mut NoProbe);
        assert_eq!(
            bits(out.velocity),
            want_velocity.map(Real::to_bits),
            "{id} velocity"
        );
        assert_eq!(bits(out.spin), want_spin.map(Real::to_bits), "{id} spin");
    }
}
