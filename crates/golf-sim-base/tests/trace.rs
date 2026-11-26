use std::cell::Cell;

use golf_sim_base::trace::{NoProbe, Phase, Probe, Source, Trace};
use serde_json::{Value, json};

fn flight_step<P: Probe>(speed: f64, probe: &mut P, energy: impl FnOnce(f64) -> f64) -> f64 {
    let speed = speed * 0.875;
    probe.begin_phase(Phase::Flight);
    probe.value("speed", speed, "m/s", Source::Computed);
    if P::ENABLED {
        probe.value(
            "specific_kinetic_energy",
            energy(speed),
            "J/kg",
            Source::Computed,
        );
    }
    speed
}

#[test]
fn probe_preserves_result_bits_and_skips_trace_only_work_when_disabled() {
    let energy_calls = Cell::new(0);
    let energy = |speed: f64| {
        energy_calls.set(energy_calls.get() + 1);
        0.5 * speed * speed
    };
    let without_trace = flight_step(58.25, &mut NoProbe, energy);
    assert_eq!(energy_calls.get(), 0);

    let mut trace = Trace::default();
    let with_trace = flight_step(58.25, &mut trace, energy);
    assert_eq!(energy_calls.get(), 1);
    assert_eq!(without_trace.to_bits(), with_trace.to_bits());
    let values = &trace.phases()[0].values;
    assert_eq!(values.len(), 2);
    assert_eq!(values[0].value.to_bits(), with_trace.to_bits());
    assert_eq!(
        values[1].value.to_bits(),
        (0.5 * with_trace * with_trace).to_bits()
    );
}

#[test]
fn json_round_trips_through_the_documented_schema() {
    let mut trace = Trace::default();
    trace.begin_phase(Phase::Delivery);
    trace.begin_phase(Phase::Impact);
    trace.begin_phase(Phase::Launch);
    trace.begin_phase(Phase::Flight);
    trace.value(
        "drag",
        0.25,
        "1",
        Source::Tunable {
            name: "AERO_DRAG".into(),
        },
    );
    let adjacent_to_one = f64::from_bits(1.0_f64.to_bits() + 1);
    trace.value("sample_time", adjacent_to_one, "s", Source::Computed);
    trace.begin_phase(Phase::Bounce);
    trace.value(
        "firmness",
        0.75,
        "1",
        Source::Turf {
            name: "fairway_firmness".into(),
        },
    );
    trace.begin_phase(Phase::Bounce);
    trace.value("lateral_speed", -0.0, "m/s", Source::Computed);
    trace.begin_phase(Phase::Skid);
    trace.begin_phase(Phase::Roll);
    trace.begin_phase(Phase::Cup);
    trace.value(
        "radius",
        0.054,
        "m",
        Source::Course {
            name: "cup_radius".into(),
        },
    );
    trace.begin_phase(Phase::Lie);
    trace.value(
        "sit_up",
        0.004,
        "m",
        Source::SeedDraw {
            name: "lie/3".into(),
        },
    );
    trace.begin_phase(Phase::Ruling);

    let json = serde_json::to_string(&trace).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&json).unwrap(),
        json!({"phases": [
            {"phase": "delivery", "values": []},
            {"phase": "impact", "values": []},
            {"phase": "launch", "values": []},
            {"phase": "flight", "values": [
                {"name": "drag", "value": 0.25, "unit": "1",
                    "source": {"kind": "tunable", "name": "AERO_DRAG"}},
                {"name": "sample_time", "value": 1.0000000000000002, "unit": "s",
                    "source": {"kind": "computed"}}
            ]},
            {"phase": "bounce", "values": [
                {"name": "firmness", "value": 0.75, "unit": "1",
                    "source": {"kind": "turf", "name": "fairway_firmness"}}
            ]},
            {"phase": "bounce", "values": [
                {"name": "lateral_speed", "value": -0.0, "unit": "m/s",
                    "source": {"kind": "computed"}}
            ]},
            {"phase": "skid", "values": []},
            {"phase": "roll", "values": []},
            {"phase": "cup", "values": [
                {"name": "radius", "value": 0.054, "unit": "m",
                    "source": {"kind": "course", "name": "cup_radius"}}
            ]},
            {"phase": "lie", "values": [
                {"name": "sit_up", "value": 0.004, "unit": "m",
                    "source": {"kind": "seed_draw", "name": "lie/3"}}
            ]},
            {"phase": "ruling", "values": []}
        ]})
    );

    let decoded: Trace = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, trace);
    assert_eq!(
        decoded.phases()[5].values[0].value.to_bits(),
        (-0.0_f64).to_bits()
    );
}

#[test]
fn json_rejects_non_finite_numbers() {
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut trace = Trace::default();
        trace.begin_phase(Phase::Flight);
        trace.value("speed", value, "m/s", Source::Computed);
        assert!(serde_json::to_string(&trace).is_err());
    }
}
