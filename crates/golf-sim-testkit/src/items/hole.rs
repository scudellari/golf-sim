use std::sync::Arc;

use golf_sim::{
    action::{Action, StrokeChoice, Swing},
    hole::Hole,
    records::hole::HoleRecord,
    setup::HoleSetup,
};
use golf_sim_base::{ClubId, PromptKind, Seed, Vec2, corpus, variant::Variant};
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

fn record_value(record: &HoleRecord) -> Variant {
    let mut record = record.clone();
    // The release jobs inject the current commit, which changes even when play is identical.
    *record.versions.get_mut("commit").unwrap() = "determinism".into();
    Variant::PackedByteArray(record.to_bytes().unwrap())
}

fn stroke(swing: Swing) -> Variant {
    let course = open_course();
    let setup = HoleSetup::standard(&course, 1, Seed(7));
    let aim = setup.tee(&course).unwrap().aim;
    let mut hole = Hole::start(course, setup).unwrap();
    let choice = StrokeChoice::stock(ClubId::Driver, aim)
        .with_aim_direction(aim)
        .with_swing(swing);
    hole.act(Action::Stroke(choice)).unwrap();
    record_value(hole.record())
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
    record_value(hole.record())
}
