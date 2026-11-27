use std::{collections::BTreeMap, sync::Arc};

use golf_sim::{
    action::ReliefOption,
    hole::Hole,
    records::hole::{DecisionKind, Entry},
};
use golf_sim_base::{
    PromptKind, Real,
    corpus::{self, Tolerance},
    variant::{Dict, Variant},
};
use golf_sim_course::course::Course;

use crate::{
    corpus::{
        FixtureRun, Result,
        projection::rows,
        rules::{self, OPTIONS, PROMPTS},
    },
    reference_records::ReferenceEntry,
};

use super::{Pair, dict, exact_stroke, get, id, integer, read_record, replay_action};

const POSITION_M: Real = 0.05;

pub(in crate::corpus) fn project(
    fixture: &mut FixtureRun,
    header: &Dict,
    drift: bool,
) -> Result<Vec<Pair>> {
    let companion = fixture.companion("rules/transitions")?;
    let mut by_record = BTreeMap::<_, Vec<_>>::new();
    for row in rows(companion.as_dict().unwrap()) {
        if let Some((record, _)) = id(row).rsplit_once("/rules.") {
            by_record.entry(record).or_default().push(row);
        }
    }
    rows(header)
        .map(|case| {
            let course = fixture.require_course(case)?;
            let transitions = by_record.get(id(case)).map_or(&[][..], Vec::as_slice);
            replay(case, &course, transitions, drift)
        })
        .collect()
}

fn replay(case: &Dict, course: &Arc<Course>, transitions: &[&Dict], drift: bool) -> Result<Pair> {
    let id = id(case);
    let reference = read_record(case, course)?;
    let mut hole = Hole::start(Arc::clone(course), reference.setup)?;
    let mut expected_fields = Vec::new();
    let mut actual_fields = Vec::new();
    let mut add = |name: String, expected: Variant, actual: Variant| {
        expected_fields.push((name.clone(), expected));
        actual_fields.push((name, actual));
    };
    let mut stopped = false;
    let mut previous_rules = None;
    for (index, entry) in reference.entries.into_iter().enumerate() {
        let at = format!("{id}/entries[{index}]");
        let (row, stroke) = match entry {
            ReferenceEntry::Stroke(facts) => {
                let row = transition(
                    transitions,
                    "rules.after_stroke",
                    |input| integer(get(input, "attempt")) == facts.attempt,
                    &at,
                )?;
                let outcome = &hole
                    .act(exact_stroke(&facts))
                    .map_err(|error| format!("{at}: {error}"))?
                    .stroke
                    .ok_or_else(|| format!("{at}: missing stroke result"))?
                    .outcome;
                let Entry::Stroke(actual) = hole.record().entries.last().unwrap() else {
                    return Err(format!("{at}: missing stroke").into());
                };
                let expected = dict(dict(row, "input"), "outcome");
                let differences = [
                    ("end", outcome.end as i64, integer(get(expected, "end"))),
                    (
                        "rest_surface",
                        outcome.rest_surface as i64,
                        integer(get(expected, "rest_surface")),
                    ),
                    (
                        "lie.surface",
                        actual.facts.lie.surface as i64,
                        facts.lie.surface as i64,
                    ),
                ]
                .into_iter()
                .filter(|&(_, actual, expected)| actual != expected)
                .map(|(field, actual, expected)| format!("{field} {expected} -> {actual}"))
                .collect::<Vec<_>>();
                if !differences.is_empty() {
                    let name = format!("stroke {}: {}", facts.attempt, differences.join(", "));
                    add(name, Variant::Bool(true), Variant::Bool(false));
                    stopped = true;
                    break;
                }
                let positions = [
                    ("rest", facts.rest, outcome.rest),
                    ("lie_spot", facts.lie.spot, actual.facts.lie.spot),
                ];
                (row, Some((facts.attempt, positions)))
            }
            ReferenceEntry::Decision(decision) => {
                let call = match decision.kind {
                    DecisionKind::GoForward => "rules.go_forward",
                    DecisionKind::Drop => "rules.after_drop",
                    DecisionKind::Place => "rules.place",
                    DecisionKind::Relief => match OPTIONS[decision.option as usize] {
                        ReliefOption::Provisional => "rules.declare_provisional",
                        ReliefOption::Embedded => "rules.embedded_relief",
                        ReliefOption::TemporaryWater
                        | ReliefOption::GroundUnderRepair
                        | ReliefOption::ImmovableObstruction
                        | ReliefOption::BunkerAbnormalOutside => "rules.ask_relief",
                        _ if hole.prompt().kind == PromptKind::Choose => "rules.choose_penalty",
                        _ => "rules.declare_unplayable",
                    },
                };
                let last_attempt = hole
                    .rules()
                    .last()
                    .and_then(|ruling| ruling.attempt)
                    .map_or(-1, |attempt| attempt as i64);
                let mut candidates = transitions.iter().copied().filter(|row| {
                    if get(row, "call").as_str() != Some(call) {
                        return false;
                    }
                    let input = dict(row, "input");
                    rules::items(get(dict(input, "rules"), "rulings"))
                        .last()
                        .map_or(-1, |ruling| {
                            integer(get(ruling.as_dict().unwrap(), "attempt"))
                        })
                        == last_attempt
                });
                let first = candidates
                    .next()
                    .ok_or_else(|| format!("{at}: missing {call} transition"))?;
                let row = std::iter::once(first)
                    .chain(candidates)
                    .find(|row| {
                        previous_rules.is_some_and(|previous| {
                            corpus::first_difference(previous, get(dict(row, "input"), "rules"))
                                .is_none()
                        })
                    })
                    .unwrap_or(first);
                let expected_ruling = Variant::Dict(decision.ruling.clone());
                hole.act(replay_action(ReferenceEntry::Decision(decision)))
                    .map_err(|error| format!("{at}: {error}"))?;
                let Entry::Decision(actual) = hole.record().entries.last().unwrap() else {
                    return Err(format!("{at}: missing decision").into());
                };
                let actual_ruling = Variant::Dict(actual.ruling.clone());
                if drift {
                    add(
                        format!("entry_{index}_ruling"),
                        expected_ruling,
                        actual_ruling,
                    );
                } else if let Some(path) = reference_difference(&expected_ruling, &actual_ruling) {
                    return Err(format!("{at}.ruling.{path}").into());
                }
                (row, None)
            }
        };
        let snapshot = dict(row, "output");
        let expected_rules = get(snapshot, "rules");
        previous_rules = Some(expected_rules);
        let actual_rules = rules::rules_value(hole.rules());
        let expected_prompt = PROMPTS[integer(get(dict(snapshot, "prompt"), "kind")) as usize];
        let actual_prompt = hole.prompt().kind;
        let rulings = rules::items(get(expected_rules.as_dict().unwrap(), "rulings"));
        let penalties: i64 = rulings
            .iter()
            .map(|ruling| integer(get(ruling.as_dict().unwrap(), "penalty_strokes")))
            .sum();
        let expected_strokes = rulings.len() as i64 + penalties;
        let actual_strokes = hole.rules().strokes() as i64;

        let discrete = if expected_prompt != actual_prompt {
            Some(format!("prompt {expected_prompt:?} -> {actual_prompt:?}"))
        } else if expected_strokes != actual_strokes {
            Some(format!("strokes {expected_strokes} -> {actual_strokes}"))
        } else {
            discrete_difference(expected_rules, &actual_rules).map(|field| format!("rules.{field}"))
        };
        if let Some(difference) = discrete {
            if let Some((attempt, _)) = stroke {
                add(
                    format!("stroke {attempt}: {difference}"),
                    Variant::Bool(true),
                    Variant::Bool(false),
                );
                stopped = true;
                break;
            }
            if !drift {
                return Err(format!("{at}: {difference}").into());
            }
            add(
                format!("entry {index}: {difference}"),
                Variant::Bool(true),
                Variant::Bool(false),
            );
        }
        if let Some((attempt, positions)) = stroke {
            for (field, expected, actual) in positions {
                let delta = (expected - actual).length();
                if drift {
                    add(
                        format!("stroke_{attempt}_{field}"),
                        rules::point_value(expected),
                        rules::point_value(actual),
                    );
                } else if delta > POSITION_M {
                    return Err(format!("{at}.{field} differs by {delta:.6} m").into());
                }
            }
        }
        if drift {
            add(
                format!("entry_{index}_rules"),
                expected_rules.clone(),
                actual_rules,
            );
        } else if let Some(path) = reference_difference(expected_rules, &actual_rules) {
            return Err(format!("{at}.rules.{path}").into());
        }
    }
    if !stopped {
        let output = dict(case, "output");
        let strokes = hole.rules().strokes() as i64;
        let penalties = hole.rules().penalties() as i64;
        if !drift
            && (strokes != integer(get(output, "strokes"))
                || penalties != integer(get(output, "penalties")))
        {
            return Err(format!("{id}: final stroke or penalty count differs").into());
        }
        add(
            "strokes".into(),
            get(output, "strokes").clone(),
            strokes.into(),
        );
        add(
            "penalties".into(),
            get(output, "penalties").clone(),
            penalties.into(),
        );
    }
    if !drift {
        assert_replays(course, &hole);
    }
    let snapshot = |fields: Vec<(String, Variant)>| {
        Variant::Dict(Dict::new(
            fields
                .into_iter()
                .map(|(name, value)| (Variant::String(name), value))
                .collect(),
        ))
    };
    Ok((snapshot(expected_fields), snapshot(actual_fields)))
}

fn transition<'a>(
    rows: &[&'a Dict],
    call: &str,
    matches: impl Fn(&Dict) -> bool,
    at: &str,
) -> Result<&'a Dict> {
    rows.iter()
        .copied()
        .find(|row| get(row, "call").as_str() == Some(call) && matches(dict(row, "input")))
        .ok_or_else(|| format!("{at}: missing {call} transition").into())
}

fn discrete_difference(expected: &Variant, actual: &Variant) -> Option<String> {
    fn discrete(value: &Variant) -> Variant {
        match value {
            Variant::Float(_)
            | Variant::Vector2(_)
            | Variant::Vector3(_)
            | Variant::PackedVector2Array(_) => Variant::Nil,
            Variant::Dict(dict) => Variant::Dict(Dict::new(
                dict.entries()
                    .iter()
                    .map(|(key, value)| (key.clone(), discrete(value)))
                    .collect(),
            )),
            Variant::Array(array) => rules::array(array.items.iter().map(discrete)),
            _ => value.clone(),
        }
    }
    corpus::first_difference(&discrete(expected), &discrete(actual))
}
fn reference_difference(expected: &Variant, actual: &Variant) -> Option<String> {
    corpus::first_difference_with(expected, actual, |path| {
        let position = [".x", ".y", ".z"].iter().any(|axis| path.ends_with(axis));
        position.then_some(Tolerance {
            absolute: POSITION_M,
            relative: 0.0,
        })
    })
}

pub fn assert_replays(course: &std::sync::Arc<Course>, hole: &Hole) {
    let bytes = hole.record().to_bytes().unwrap();
    let replay = Hole::resume(course.clone(), &bytes).unwrap();
    assert_eq!(replay.record().to_bytes().unwrap(), bytes);
    assert_eq!(hole.rules(), replay.rules());
    assert_eq!(hole.ball_lie(), replay.ball_lie());
    assert_eq!(hole.clock_s().to_bits(), replay.clock_s().to_bits());
}
