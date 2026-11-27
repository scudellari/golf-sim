use std::sync::Arc;

use golf_sim::{
    GOLF_SIM_COMMIT,
    action::{Action, StrokeChoice, Swing},
    hole::Hole,
    records::hole::HoleRecord,
    setup::HoleSetup,
};
use golf_sim_base::{
    ClubId, PromptKind, Seed, Vec2, corpus,
    variant::{self, Variant},
};
use golf_sim_course::course::Course;
use golf_sim_swing::hands::Profile;

use crate::{
    determinism::{Item, fixture_dir},
    reference_records::{ReferenceEntry, decode_hole},
};

pub fn items() -> Vec<Item> {
    vec![
        Item::new("record.played_hole", played_hole),
        Item::new("stroke.exact", || {
            stroke(Swing::Exact {
                power: 0.85,
                timing_fault: -0.25,
                strike_m: Some(Vec2::new(0.002, -0.001)),
            })
        }),
        Item::new("stroke.scratch", || stroke(Swing::Skill(Profile::SCRATCH))),
        Item::new("stroke.hcp15", || stroke(Swing::Skill(Profile::HCP15))),
    ]
}

fn open_course() -> Arc<Course> {
    Course::open(&fixture_dir().join("../courses/test-course")).unwrap()
}

/// Mask the build stamp in a hole record or a single-stroke repro.
pub fn masked_record_bytes(bytes: &[u8]) -> Vec<u8> {
    with_stamps(
        bytes,
        &[("release", "determinism"), ("commit", "determinism")],
    )
}

/// Restamp a fixture copy to replay its saved results against the current physics.
pub fn record_for_current_build(bytes: &[u8]) -> Vec<u8> {
    with_stamps(
        bytes,
        &[
            ("release", env!("CARGO_PKG_VERSION")),
            ("commit", GOLF_SIM_COMMIT),
            ("physics", golf_sim_physics::tunables::VERSION),
        ],
    )
}

fn with_stamps(bytes: &[u8], stamps: &[(&str, &str)]) -> Vec<u8> {
    let (magic, rest) = bytes.split_first_chunk::<8>().unwrap();
    assert!(matches!(magic, b"LLHOLE01" | b"GSSTROK1"));
    let (size, body) = rest.split_first_chunk::<4>().unwrap();
    assert_eq!(body.len(), u32::from_le_bytes(*size) as usize);
    let mut record = variant::decode(body).unwrap();
    let Variant::Dict(dict) = &mut record else {
        panic!("expected a record dictionary");
    };
    let Some(Variant::Dict(versions)) = dict.get_mut("versions") else {
        panic!("expected record versions");
    };
    for &(name, stamp) in stamps {
        let value = versions.get_mut(name).unwrap();
        assert!(value.as_str().is_some(), "expected a string for {name}");
        *value = stamp.into();
    }
    let body = variant::encode(&record).unwrap();
    let size = u32::try_from(body.len()).unwrap().to_le_bytes();
    [magic.as_slice(), &size, &body].concat()
}

fn record_value(bytes: &[u8]) -> Variant {
    Variant::PackedByteArray(masked_record_bytes(bytes))
}

fn stroke(swing: Swing) -> Variant {
    record_value(&stroke_record(swing).to_bytes().unwrap())
}

fn stroke_record(swing: Swing) -> HoleRecord {
    let course = open_course();
    let setup = HoleSetup::standard(&course, 1, Seed(7));
    let aim = setup.tee(&course).unwrap().aim;
    let mut hole = Hole::start(course, setup).unwrap();
    let choice = StrokeChoice::stock(ClubId::Driver, aim)
        .with_aim_direction(aim)
        .with_swing(swing);
    hole.act(Action::Stroke(choice)).unwrap();
    hole.record().clone()
}

fn played_hole() -> Variant {
    let course = open_course();
    let fixture = corpus::read(&fixture_dir().join("../records/holes/fixture.bin")).unwrap();
    let Some(Variant::Array(cases)) = fixture.as_dict().unwrap().get("cases") else {
        panic!("expected hole cases");
    };
    let case = cases
        .items
        .iter()
        .filter_map(Variant::as_dict)
        .find(|case| {
            case.get("id").and_then(Variant::as_str) == Some("play/test-course/2/1/caddie")
        })
        .unwrap();
    let output = case.get("output").and_then(Variant::as_dict).unwrap();
    let Some(Variant::PackedByteArray(bytes)) = output.get("bytes") else {
        panic!("expected hole record");
    };
    let reference = decode_hole(bytes, &course.data().id).unwrap();
    let mut hole = Hole::start(course, reference.setup).unwrap();
    for entry in reference.entries {
        let ReferenceEntry::Stroke(facts) = entry else {
            panic!("this hole is played without a relief decision");
        };
        let execution = facts.execution;
        let mut choice = StrokeChoice::stock(facts.club_id, facts.aim)
            .with_aim_direction(facts.aim)
            .with_swing(Swing::Exact {
                power: execution.power,
                timing_fault: execution.timing_fault,
                strike_m: execution.strike_m,
            });
        choice.address = facts.address;
        choice.flagstick_in = Some(facts.flagstick_in);
        hole.act(Action::Stroke(choice)).unwrap();
    }
    assert_eq!(hole.rules().prompt().kind, PromptKind::Done);
    record_value(&hole.record().to_bytes().unwrap())
}

#[cfg(test)]
mod tests {
    use golf_sim_base::digest::digest;

    use super::*;

    #[test]
    fn release_and_commit_bumps_preserve_masked_record_bytes_and_digest() {
        let mut record = stroke_record(Swing::Skill(Profile::HCP15));
        let current = record.to_bytes().unwrap();
        *record.versions.get_mut("release").unwrap() = "1.2.3".into();
        *record.versions.get_mut("commit").unwrap() = "abc".into();
        let original = record.to_bytes().unwrap();
        *record.versions.get_mut("release").unwrap() = "9.9.9".into();
        *record.versions.get_mut("commit").unwrap() = "deadbeef".into();
        let rewritten = record.to_bytes().unwrap();

        assert_ne!(original, rewritten);
        assert_eq!(
            masked_record_bytes(&current),
            masked_record_bytes(&original)
        );
        assert_eq!(
            masked_record_bytes(&original),
            masked_record_bytes(&rewritten)
        );
        let original_digest = digest(&record_value(&original)).unwrap();
        assert_eq!(original_digest, digest(&record_value(&rewritten)).unwrap());

        for model in ["physics", "tuning", "rules"] {
            let mut changed = record.clone();
            *changed.versions.get_mut(model).unwrap() = "9.9.9".into();
            let bytes = changed.to_bytes().unwrap();
            assert_ne!(
                original_digest,
                digest(&record_value(&bytes)).unwrap(),
                "{model}"
            );
        }
    }
}
