use std::{collections::BTreeMap, sync::Arc};

use golf_sim::{
    action::{Action, StrokeChoice, Swing},
    hole::Hole,
    records::hole::{DecisionKind, StrokeFacts},
};
use golf_sim_base::{
    PromptKind, Vec3,
    variant::{Dict, Variant},
};
use golf_sim_course::course::Course;

use crate::reference_records::{ReferenceEntry, ReferenceHoleRecord, decode_hole};

use super::{
    FixtureRun, Result, Runner,
    projection::{Pair, id, rows},
    rules::OPTIONS,
};

mod advice;
mod observe;
mod records;

pub use observe::assert_last_stroke;
pub use records::assert_replays;
pub(super) use records::project as records;

pub(super) fn addresses(fixture: &mut FixtureRun, header: &Dict, drift: bool) -> Result<Vec<Pair>> {
    let companion = fixture.companion("records/holes")?;
    let records: BTreeMap<_, _> = rows(companion.as_dict().unwrap())
        .map(|record| (id(record), record))
        .collect();
    let mut by_record = BTreeMap::<_, BTreeMap<_, Vec<_>>>::new();
    for (index, case) in rows(header).enumerate() {
        let (record, address) = id(case)
            .rsplit_once("/address/")
            .ok_or_else(|| format!("{}: expected a record id and address index", id(case)))?;
        by_record
            .entry(record)
            .or_default()
            .entry(address.parse::<usize>()?)
            .or_default()
            .push((index, case));
    }

    let mut outputs = BTreeMap::new();
    for (record, mut addresses) in by_record {
        let source = records
            .get(record)
            .ok_or_else(|| format!("missing record {record:?}"))?;
        let course = fixture.require_course(addresses.values().next().unwrap()[0].1)?;
        let reference = read_record(source, &course)?;
        let mode = reference.setup.settings.info_mode;
        let mut hole = Hole::start(Arc::clone(&course), reference.setup)?;
        let mut attempt = 0;
        let mut address = 0;
        let mut last_stroke = None;
        for (index, entry) in reference
            .entries
            .into_iter()
            .map(Some)
            .chain([None])
            .enumerate()
        {
            let prefix = format!("{record}/prefix/{index}");
            let action = entry.map(replay_action);
            if fixture.runner == Runner::HoleAdvice && !drift {
                advice::check_actions(&mut hole, &course, action.as_ref(), &prefix);
            }
            if hole.prompt().kind == PromptKind::Play {
                for (index, case) in addresses.remove(&address).into_iter().flatten() {
                    let case_course = fixture.require_course(case)?;
                    if !Arc::ptr_eq(&case_course, &course) {
                        return Err(format!("{}: course differs from its record", id(case)).into());
                    }
                    let input = dict(case, "input");
                    assert_eq!(integer(get(input, "attempt")), attempt, "{}", id(case));
                    assert_eq!(
                        integer(get(input, "info_mode")),
                        mode as i64,
                        "{}",
                        id(case)
                    );
                    let pair = if fixture.runner == Runner::HoleAdvice {
                        advice::run(&hole, &course, case, mode, drift)
                    } else {
                        observe::run(&hole, case, mode, last_stroke.as_ref())
                    };
                    outputs.insert(index, pair);
                }
                address += 1;
            }
            if let Some(action) = action {
                attempt += i64::from(matches!(action, Action::Stroke(_)));
                let step = hole
                    .act(action)
                    .map_err(|error| format!("{prefix}: {error}"))?;
                if let Some(stroke) = step.stroke {
                    last_stroke = Some(stroke);
                }
            }
        }
        if let Some((_, case)) = addresses.values().flatten().next() {
            return Err(format!("unmatched address {}", id(case)).into());
        }
    }
    Ok(outputs.into_values().collect())
}

fn read_record(case: &Dict, course: &Course) -> Result<ReferenceHoleRecord> {
    let Variant::PackedByteArray(bytes) = get(dict(case, "output"), "bytes") else {
        return Err(format!("{}: expected record bytes", id(case)).into());
    };
    Ok(decode_hole(bytes, &course.data().id)?)
}

fn exact_stroke(facts: &StrokeFacts) -> Action {
    let mut choice = StrokeChoice::stock(facts.club_id, facts.aim)
        .with_aim_direction(facts.aim)
        .with_swing(Swing::Exact {
            power: facts.execution.power,
            timing_fault: facts.execution.timing_fault,
            strike_m: facts.execution.strike_m,
        });
    choice.address = facts.address.clone();
    choice.flagstick_in = Some(facts.flagstick_in);
    Action::Stroke(choice)
}

fn replay_action(entry: ReferenceEntry) -> Action {
    match entry {
        ReferenceEntry::Stroke(facts) => exact_stroke(&facts),
        ReferenceEntry::Decision(decision) => match decision.kind {
            DecisionKind::GoForward => Action::GoForward,
            DecisionKind::Drop => Action::Drop(decision.point),
            DecisionKind::Place => Action::Place(decision.point),
            DecisionKind::Relief => Action::Relief {
                option: OPTIONS[decision.option as usize],
                footprint: decision.footprint,
            },
        },
    }
}

fn get<'a>(dict: &'a Dict, key: &str) -> &'a Variant {
    dict.get(key).unwrap_or_else(|| panic!("missing {key}"))
}

fn dict<'a>(fields: &'a Dict, key: &str) -> &'a Dict {
    get(fields, key).as_dict().unwrap()
}

fn integer(value: &Variant) -> i64 {
    let Variant::Int(number) = value else {
        panic!("expected integer");
    };
    number.value()
}

fn vector(dict: &Dict, key: &str) -> Vec3 {
    let Variant::Vector3(vector) = get(dict, key) else {
        panic!("{key}: expected vector");
    };
    let [x, y, z] = vector.values;
    Vec3::new(x, y, z)
}
