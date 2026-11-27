use std::{convert::Infallible, path::Path};

use golf_sim_base::{
    Real, Seed, Vec2, fm,
    num::roundi,
    seed::SeedStream,
    variant::{Components, Dict, FloatWidth, Variant},
};
use golf_sim_swing::{
    execution::Execution,
    hands::{Profile, Swing, exact, skill},
    spec::{StrikeMode, SwingSpec},
};
use golf_sim_testkit::{corpus::FixtureRun, tolerance};

#[test]
fn exact_swings_match_recorded_power_and_timing() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/swing/streams/fixture.bin");
    let fixture = FixtureRun::read(&path, Some("swing"), &[]).unwrap();
    let is_golfer =
        |input: &Dict| input.get("golfer").and_then(Variant::as_str) == Some("hole_kit");
    let recorded_strike = |output: &Dict| {
        let recorded = dict(output, "execution");
        let Variant::Vector2(strike) = recorded.get("strike_m").unwrap() else {
            panic!("expected a strike vector");
        };
        Vec2::new(strike.values[0], strike.values[1])
    };
    let (expected_golfer_cases, expected_off_centre) = fixture
        .cases()
        .filter(|case| is_golfer(dict(case, "input")))
        .fold((0, 0), |(total, off_centre), case| {
            let strike = recorded_strike(dict(case, "output"));
            (total + 1, off_centre + usize::from(strike != Vec2::ZERO))
        });
    let mut golfer_cases = 0;
    let mut off_centre = 0;
    let mut largest_strike_m: Real = 0.0;
    let report = tolerance::check(&path, "swing", |case| {
        let input = dict(case, "input");
        let mut output = dict(case, "output").clone();
        if !is_golfer(input) {
            return Ok::<_, Infallible>(Variant::Dict(output));
        }
        golfer_cases += 1;
        let id = case.get("id").and_then(Variant::as_str).unwrap();
        let spec = exact_spec(dict(input, "spec"));
        let start_usec = integer(input, "start_usec");
        let recorded = dict(&output, "execution");
        let power = number(recorded, "power");
        let centre = (spec.strike_mode == StrikeMode::Reticle).then_some(Vec2::ZERO);
        let mut actual = exact(
            &spec,
            Swing::Exact {
                power,
                timing_fault: 0.0,
                strike_m: centre,
            },
            start_usec,
        );
        // The reference press lands on the same rounded microsecond as the ring closes.
        assert_eq!(number(recorded, "timing_fault"), 0.0, "{id}");
        assert_eq!(actual.impact_usec, integer(recorded, "impact_usec"), "{id}");
        assert_eq!(actual.strike_m, centre, "{id}");

        let recorded_strike = recorded_strike(&output);
        if recorded_strike != Vec2::ZERO {
            assert!(centre.is_some(), "{id}: strike input disabled");
            off_centre += 1;
            largest_strike_m = largest_strike_m.max(recorded_strike.length());
            // The reference golfer leaves the reticle alone, so its seeded drift is recorded.
            actual = exact(
                &spec,
                Swing::Exact {
                    power,
                    timing_fault: 0.0,
                    strike_m: Some(recorded_strike),
                },
                start_usec,
            );
        }
        *output.get_mut("execution").unwrap() = snapshot(actual);
        Ok(Variant::Dict(output))
    })
    .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(report.checked, fixture.cases().len());
    assert_eq!(golfer_cases, expected_golfer_cases);
    assert_eq!(off_centre, expected_off_centre);
    assert!(report.skipped.is_empty(), "{report}");
    assert!(report.discrete.is_empty(), "{report}");
    println!("{golfer_cases} exact swings matched recorded power and timing");
    println!(
        "{} matched with strike input disabled. {off_centre} matched using the recorded off-centre strike (maximum {:.3} mm)",
        golfer_cases - off_centre,
        largest_strike_m * 1_000.0,
    );
}

#[test]
fn exact_fault_uses_the_window_on_its_side() {
    let spec = SwingSpec {
        meter_rate_per_s: 1.0,
        window_early_s: 0.010,
        window_late_s: 0.030,
        ..SwingSpec::default()
    };
    let strike = Some(Vec2::new(0.006, -0.002));
    for (fault, error_s, stamp) in [(-0.5, -0.005, 1_795_001), (0.5, 0.015, 1_815_001)] {
        let execution = exact(
            &spec,
            Swing::Exact {
                power: 0.600_001,
                timing_fault: fault,
                strike_m: strike,
            },
            1_000_000,
        );
        assert_eq!(execution.timing_error_s, error_s);
        assert_eq!(execution.timing_fault, fault);
        assert_eq!(execution.impact_usec, stamp);
        assert_eq!(execution.strike_m, strike);
        assert!(!execution.timed_out);
    }
}

#[test]
fn the_same_profile_faults_more_in_a_narrower_window() {
    const SEEDS: i64 = 1_024;
    let wide = SwingSpec {
        strike_mode: StrikeMode::None,
        ..SwingSpec::default()
    };
    let narrow = SwingSpec {
        window_early_s: wide.window_early_s / 2.0,
        window_late_s: wide.window_late_s / 2.0,
        ..wide
    };
    let swing = Swing::Skill(Profile::HCP15);
    let mean_fault = |spec| {
        (0..SEEDS)
            .map(|seed| swing.execute(spec, 250_000, Seed(seed)).timing_fault.abs())
            .sum::<Real>()
            / SEEDS as Real
    };
    let wide_mean = mean_fault(&wide);
    let narrow_mean = mean_fault(&narrow);
    let comparison = format!(
        "mean absolute timing fault over {SEEDS} seeds: wide {wide_mean:.6}, narrow {narrow_mean:.6}"
    );
    assert!(narrow_mean > wide_mean, "{comparison}");
    println!("{comparison}");
}

#[test]
fn profile_and_seed_repeat_every_execution_bit() {
    let spec = SwingSpec {
        steer_vertical: true,
        ..SwingSpec::default()
    };
    for profile in [Profile::SCRATCH, Profile::HCP15] {
        let swing = Swing::Skill(profile);
        for seed in 0..512 {
            let bytes = |execution| golf_sim_base::variant::encode(&snapshot(execution)).unwrap();
            let execution = swing.execute(&spec, 250_000, Seed(seed));
            assert_eq!(
                bytes(execution),
                bytes(swing.execute(&spec, 250_000, Seed(seed))),
                "{} seed {seed}",
                profile.name,
            );
        }
    }
    println!("1,024 skill executions repeated bit for bit across two profiles");
}

#[test]
fn skill_draw_order_and_steer_lag_reach_the_session() {
    let profile = Profile {
        bias_t_ms: -18.0,
        ..Profile::SCRATCH
    };
    let spec = SwingSpec {
        target_power: 0.1,
        meter_rate_per_s: 1.0,
        drift_start_sd_m: Vec2::ZERO,
        drift_slide_sd_mps: Vec2::ZERO,
        wobble_m: Vec2::ZERO,
        steer_vertical: true,
        ..SwingSpec::default()
    };
    let seed = Seed(-72);
    let wanted = Vec2::new(-0.003, 0.002);
    let start_usec = 250_000;
    let execution = skill(&spec, profile, wanted, start_usec, seed);
    let mut draws = SeedStream::new(seed.derive("hands"));
    let power_usec = roundi(100_000.0 + 12_000.0 * draws.next_gaussian());
    let timing_usec = roundi(-18_000.0 + 6_000.0 * draws.next_gaussian());
    let target = Vec2::new(
        wanted.x + 0.002 * draws.next_gaussian(),
        wanted.y + 0.002 * draws.next_gaussian(),
    );
    let elapsed_usec = power_usec + 200_000 + timing_usec;
    let steer_elapsed_ms = (elapsed_usec / 10_000 * 10) as Real;
    let expected_strike = target * (1.0 - fm::exp(-steer_elapsed_ms / 50.0));
    assert_eq!(execution.impact_usec, start_usec + elapsed_usec);
    assert_eq!(execution.power, power_usec as Real / 1_000_000.0);
    assert_eq!(execution.timing_error_s, timing_usec as Real / 1_000_000.0);
    assert!((execution.strike_m.unwrap() - expected_strike).length() < 1e-15);
}

#[test]
fn a_skill_power_press_past_cap_leaves_the_impact_press_to_the_ring() {
    let spec = SwingSpec {
        target_power: 1.2,
        ..SwingSpec::default()
    };
    let mut profile = Profile::SCRATCH;
    let swing = |profile| Swing::Skill(profile).execute(&spec, 250_000, Seed(7));
    let execution = swing(profile);
    assert_eq!(execution.power, spec.power_cap);
    assert!(execution.timing_error_s.abs() < 0.03);
    assert!(!execution.timed_out);

    profile.bias_t_ms = 1_000.0;
    let execution = swing(profile);
    assert_eq!(execution.power, spec.power_cap);
    assert_eq!(execution.timing_error_s, spec.timeout_after_s);
    assert!(execution.timed_out);
}

fn exact_spec(input: &Dict) -> SwingSpec {
    SwingSpec {
        meter_rate_per_s: number(input, "meter_rate_per_s"),
        tempo_ratio: number(input, "tempo_ratio"),
        ring_min_s: number(input, "ring_min_s"),
        ring_max_s: number(input, "ring_max_s"),
        window_early_s: number(input, "window_early_s"),
        window_late_s: number(input, "window_late_s"),
        strike_mode: match integer(input, "strike_mode") {
            0 => StrikeMode::Reticle,
            1 => StrikeMode::None,
            value => panic!("unknown strike mode {value}"),
        },
        ..SwingSpec::default()
    }
}

fn dict<'a>(input: &'a Dict, key: &str) -> &'a Dict {
    input.get(key).unwrap().as_dict().unwrap()
}

fn number(input: &Dict, key: &str) -> Real {
    match input.get(key).unwrap() {
        Variant::Float(value) => value.value(),
        Variant::Int(value) => value.value() as Real,
        _ => panic!("expected number at {key}"),
    }
}

fn integer(input: &Dict, key: &str) -> i64 {
    match input.get(key).unwrap() {
        Variant::Int(value) => value.value(),
        _ => panic!("expected integer at {key}"),
    }
}

fn snapshot(execution: Execution) -> Variant {
    let strike = execution.strike_m.unwrap_or(Vec2::ZERO);
    golf_sim_base::digest::object([
        ("power", execution.power.into()),
        ("timing_error_s", execution.timing_error_s.into()),
        ("timing_fault", execution.timing_fault.into()),
        ("timed_out", Variant::Bool(execution.timed_out)),
        ("has_strike", Variant::Bool(execution.strike_m.is_some())),
        (
            "strike_m",
            Variant::Vector2(Components {
                values: [strike.x, strike.y],
                width: FloatWidth::Double,
            }),
        ),
        ("impact_usec", execution.impact_usec.into()),
        ("backswing_s", execution.backswing_s.into()),
        ("ring_s", execution.ring_s.into()),
    ])
}
