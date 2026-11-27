use golf_sim_base::{
    Seed, Vec2,
    digest::object,
    variant::{Components, FloatWidth, Variant},
};
use golf_sim_swing::{
    execution::Execution,
    hands::{Profile, Swing},
    spec::{StrikeMode, SwingSpec},
};

use crate::determinism::Item;

pub fn items() -> Vec<Item> {
    vec![
        Item::new("exact", || {
            snapshot(
                Swing::Exact {
                    power: 0.78,
                    timing_fault: -0.75,
                    strike_m: Some(Vec2::new(-0.002, 0.004)),
                }
                .execute(&tee(), 275_000, Seed(7)),
            )
        }),
        Item::new("scratch.tee", || skill(Profile::SCRATCH, tee())),
        Item::new("scratch.rough", || skill(Profile::SCRATCH, rough())),
        Item::new("scratch.putt", || skill(Profile::SCRATCH, putt())),
        Item::new("hcp15.tee", || skill(Profile::HCP15, tee())),
        Item::new("hcp15.rough", || skill(Profile::HCP15, rough())),
        Item::new("hcp15.putt", || skill(Profile::HCP15, putt())),
    ]
}

fn tee() -> SwingSpec {
    SwingSpec {
        window_early_s: 0.040,
        window_late_s: 0.035,
        drift_start_sd_m: Vec2::new(0.007, 0.002),
        steer_vertical: true,
        ..SwingSpec::default()
    }
}

fn rough() -> SwingSpec {
    SwingSpec {
        target_power: 0.65,
        window_early_s: 0.012,
        window_late_s: 0.010,
        drift_bias_m: Vec2::new(-0.004, 0.0),
        ..SwingSpec::default()
    }
}

fn putt() -> SwingSpec {
    SwingSpec {
        target_power: 0.28,
        strike_mode: StrikeMode::None,
        ..SwingSpec::default()
    }
}

fn skill(profile: Profile, spec: SwingSpec) -> Variant {
    snapshot(Swing::Skill(profile).execute(&spec, 275_000, Seed(7)))
}

fn snapshot(execution: Execution) -> Variant {
    let strike = execution.strike_m.map_or(Variant::Nil, |strike| {
        Variant::Vector2(Components {
            values: [strike.x, strike.y],
            width: FloatWidth::Double,
        })
    });
    object([
        ("power", execution.power.into()),
        ("timing_error_s", execution.timing_error_s.into()),
        ("timing_fault", execution.timing_fault.into()),
        ("timed_out", Variant::Bool(execution.timed_out)),
        ("strike_m", strike),
        ("impact_usec", execution.impact_usec.into()),
        ("backswing_s", execution.backswing_s.into()),
        ("ring_s", execution.ring_s.into()),
    ])
}
