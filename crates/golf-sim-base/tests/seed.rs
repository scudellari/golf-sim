use std::{collections::BTreeMap, path::Path};

use golf_sim_base::{
    Real, Seed, corpus,
    seed::SeedStream,
    variant::{Dict, Variant},
};

#[test]
fn seed_streams_match_the_reference() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/seed/streams/fixture.bin");
    let fixture = corpus::read(&path).unwrap();
    let fixture = fixture.as_dict().unwrap();
    assert_eq!(string(fixture, "layer"), "seed/streams");

    let mut counts = BTreeMap::new();
    let mut worst_absolute: Real = 0.0;
    let mut worst_scaled: Real = 0.0;
    for case in array(fixture, "cases") {
        let case = case.as_dict().unwrap();
        let id = string(case, "id");
        let op = string(case, "op");
        *counts.entry(op).or_insert(0) += 1;
        match op {
            "derive" => {
                let parent = Seed(integer(field(case, "parent")));
                let expected = Seed(integer(field(case, "out")));
                assert_eq!(parent.derive(string(case, "label")), expected, "{id}");
                continue;
            }
            "stream" => {}
            _ => panic!("{id} unknown case operation {op}"),
        }

        let mut stream = SeedStream::new(Seed(integer(field(case, "seed"))));
        let calls = array(case, "calls");
        let outputs = array(case, "out");
        assert_eq!(calls.len(), outputs.len(), "{id} output count");
        for (index, (call, expected)) in calls.iter().zip(outputs).enumerate() {
            let call = call.as_dict().unwrap();
            let op = string(call, "op");
            *counts.entry(op).or_insert(0) += 1;
            let actual = match op {
                "next_int" => {
                    assert_eq!(stream.next_int(), integer(expected), "{id} calls[{index}]");
                    continue;
                }
                "next_float" => stream.next_float(),
                "next_range" => {
                    stream.next_range(real(field(call, "low")), real(field(call, "high")))
                }
                "next_gaussian" => {
                    let actual = stream.next_gaussian();
                    let expected = real(expected);
                    let error = (actual - expected).abs();
                    let scale = expected.abs().max(1.0);
                    assert!(
                        error <= 1e-12 * scale,
                        "{id} calls[{index}] {op} expected {expected}, got {actual} (error {error:e})"
                    );
                    worst_absolute = worst_absolute.max(error);
                    worst_scaled = worst_scaled.max(error / scale);
                    continue;
                }
                _ => panic!("{id} calls[{index}] unknown stream operation {op}"),
            };
            assert_eq!(
                actual.to_bits(),
                real(expected).to_bits(),
                "{id} calls[{index}] {op}"
            );
        }
    }

    assert_eq!(
        counts,
        BTreeMap::from([
            ("derive", 8_000),
            ("stream", 2_000),
            ("next_int", 2_800),
            ("next_float", 2_800),
            ("next_range", 3_200),
            ("next_gaussian", 4_000),
        ])
    );
    println!(
        "Exact matches: {} derivations, {} integers, {} floats, {} ranges",
        counts["derive"], counts["next_int"], counts["next_float"], counts["next_range"]
    );
    println!(
        "{} Gaussians within tolerance. Worst absolute error {worst_absolute:.9e}, worst scaled error {worst_scaled:.9e}",
        counts["next_gaussian"]
    );
}

#[test]
fn derived_stream_starts_at_the_child_seed() {
    let parent = Seed(-7);
    let mut derived = SeedStream::derived(parent, "hands");
    let mut child = SeedStream::new(parent.derive("hands"));
    assert_eq!(derived.next_int(), child.next_int());
}

#[test]
fn sign_uses_one_uniform_draw() {
    let mut stream = SeedStream::default();
    let mut zero_seed = SeedStream::new(Seed(0));
    for expected in [-1.0, 1.0] {
        assert_eq!(stream.next_sign(), expected);
        zero_seed.next_float();
    }
    assert_eq!(stream.next_int(), zero_seed.next_int());
}

fn field<'a>(dict: &'a Dict, key: &str) -> &'a Variant {
    dict.get(key)
        .unwrap_or_else(|| panic!("missing field {key}"))
}

fn string<'a>(dict: &'a Dict, key: &str) -> &'a str {
    field(dict, key).as_str().unwrap()
}

fn array<'a>(dict: &'a Dict, key: &str) -> &'a [Variant] {
    let Variant::Array(array) = field(dict, key) else {
        panic!("expected an array for {key}");
    };
    &array.items
}

fn integer(value: &Variant) -> i64 {
    let Variant::Int(value) = value else {
        panic!("expected an integer, got {value:?}");
    };
    value.value()
}

fn real(value: &Variant) -> Real {
    let Variant::Float(value) = value else {
        panic!("expected a float, got {value:?}");
    };
    value.value()
}
