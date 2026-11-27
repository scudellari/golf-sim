use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use golf_sim::{
    action::{Action, ReliefOption, StrokeChoice, Swing},
    hole::Hole,
    records::hole::{DecisionKind, Entry, HoleRecord, Refusal, StrokeEntry, SwingPress},
    setup::{HoleSetup, StrikeVariant},
};
use golf_sim_base::{
    ClubId, Error, PromptKind, Real, Seed, Vec2, Vec3,
    corpus::{self, Tolerance},
    digest::object,
    trace::Phase,
    variant::{Array, Components, Dict, FloatWidth, Variant},
};
use golf_sim_course::course::{Course, PenaltyColor};
use golf_sim_rules::rulings::{HoleRules, Ruling};
use golf_sim_stroke::{
    address::{Address, Setting},
    delivery::DeliveryInput,
    stroke,
};
use golf_sim_swing::hands::Profile;
use golf_sim_swing::session::SwingSession;
use golf_sim_testkit::reference_records::{ReferenceEntry, decode_hole};

const NO_POINT: Vec3 = Vec3::new(Real::INFINITY, Real::INFINITY, Real::INFINITY);
const POSITION_M: Real = 0.05;
const PROMPTS: [PromptKind; 6] = [
    PromptKind::Play,
    PromptKind::MaybeLost,
    PromptKind::Done,
    PromptKind::Choose,
    PromptKind::Drop,
    PromptKind::Place,
];
const OPTIONS: [ReliefOption; 11] = {
    use ReliefOption::*;
    [
        Cancel,
        StrokeAndDistance,
        BackOnLine,
        Lateral,
        OutsideBunker,
        TemporaryWater,
        GroundUnderRepair,
        ImmovableObstruction,
        Embedded,
        BunkerAbnormalOutside,
        Provisional,
    ]
};

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

fn dict<'a>(parent: &'a Dict, key: &str) -> &'a Dict {
    parent.get(key).unwrap().as_dict().unwrap()
}

fn items(value: &Variant) -> &[Variant] {
    let Variant::Array(array) = value else {
        panic!("expected array")
    };
    &array.items
}

fn integer(value: &Variant) -> i64 {
    let Variant::Int(value) = value else {
        panic!("expected integer")
    };
    value.value()
}

fn cases(path: &str) -> Vec<Variant> {
    let fixture = corpus::read(&testdata().join(path)).unwrap();
    items(fixture.as_dict().unwrap().get("cases").unwrap()).to_vec()
}

fn transition_rows(rows: &[Variant]) -> BTreeMap<&str, Vec<&Dict>> {
    let mut by_hole = BTreeMap::<_, Vec<_>>::new();
    for row in rows {
        let row = row.as_dict().unwrap();
        let id = row.get("id").unwrap().as_str().unwrap();
        if id.starts_with("play/") {
            let (hole, _) = id.rsplit_once('/').unwrap().0.rsplit_once('/').unwrap();
            by_hole.entry(hole).or_default().push(row);
        }
    }
    by_hole
}

#[test]
fn reference_holes() {
    let course = Course::open(&testdata().join("courses/test-course")).unwrap();
    let transitions = cases("rules/transitions/fixture.bin");
    let by_hole = transition_rows(&transitions);
    let cases = cases("records/holes/fixture.bin");
    assert_eq!(cases.len(), 30);
    let allowed_share = fs::read_to_string(testdata().join("tolerances.toml"))
        .unwrap()
        .split_once("[hole]\n")
        .unwrap()
        .1
        .split("\n[")
        .next()
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("discrete_share = "))
        .unwrap()
        .parse::<Real>()
        .unwrap();
    let mut discrete = Vec::new();
    let mut failed = Vec::new();
    let mut strokes = 0;
    let mut decisions = 0;
    for case in &cases {
        let case = case.as_dict().unwrap();
        let id = case.get("id").unwrap().as_str().unwrap();
        let output = dict(case, "output");
        let Some(Variant::PackedByteArray(bytes)) = output.get("bytes") else {
            panic!("expected record bytes");
        };
        let reference = decode_hole(bytes, &course.data().id).unwrap();
        let mut hole = Hole::start(course.clone(), reference.setup).unwrap();
        let rows = &by_hole[id];
        let mut compared = true;
        for (index, entry) in reference.entries.into_iter().enumerate() {
            let at = format!("{id}/entries[{index}]");
            let check = match entry {
                ReferenceEntry::Stroke(facts) => {
                    let row = rows
                        .iter()
                        .copied()
                        .find(|row| {
                            row.get("call").and_then(Variant::as_str) == Some("rules.after_stroke")
                                && integer(dict(row, "input").get("attempt").unwrap())
                                    == facts.attempt
                        })
                        .unwrap_or_else(|| panic!("{at}: missing stroke transition"));
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
                    let result = hole
                        .act(Action::Stroke(choice))
                        .unwrap_or_else(|error| panic!("{at}: {error}"))
                        .stroke
                        .unwrap();
                    let expected = dict(dict(row, "input"), "outcome");
                    let Entry::Stroke(actual) = hole.record().entries.last().unwrap() else {
                        panic!("{at}: missing stroke");
                    };
                    let fields = [
                        (
                            "end",
                            result.outcome.end as i64,
                            integer(expected.get("end").unwrap()),
                        ),
                        (
                            "rest_surface",
                            result.outcome.rest_surface as i64,
                            integer(expected.get("rest_surface").unwrap()),
                        ),
                        (
                            "lie.surface",
                            actual.facts.lie.surface as i64,
                            facts.lie.surface as i64,
                        ),
                    ];
                    let differences: Vec<_> = fields
                        .into_iter()
                        .filter(|&(_, actual, expected)| actual != expected)
                        .map(|(field, actual, expected)| format!("{field} {actual} vs {expected}"))
                        .collect();
                    if !differences.is_empty() {
                        discrete.push(format!(
                            "{id}/stroke/{}: {}",
                            facts.attempt,
                            differences.join(", ")
                        ));
                        compared = false;
                        break;
                    }
                    for (field, expected, actual) in [
                        ("rest", facts.rest, result.outcome.rest),
                        ("lie.spot", facts.lie.spot, actual.facts.lie.spot),
                    ] {
                        let delta = (expected - actual).length();
                        if delta > POSITION_M {
                            failed.push(format!("{at}.{field} differs by {delta:.6} m"));
                        }
                    }
                    strokes += 1;
                    row
                }
                ReferenceEntry::Decision(decision) => {
                    let (call, action) = match decision.kind {
                        DecisionKind::GoForward => ("rules.go_forward", Action::GoForward),
                        DecisionKind::Drop => ("rules.after_drop", Action::Drop(decision.point)),
                        DecisionKind::Place => ("rules.place", Action::Place(decision.point)),
                        DecisionKind::Relief => {
                            let option = OPTIONS[decision.option as usize];
                            let call = match option {
                                ReliefOption::Provisional => "rules.declare_provisional",
                                ReliefOption::Embedded => "rules.embedded_relief",
                                ReliefOption::TemporaryWater
                                | ReliefOption::GroundUnderRepair
                                | ReliefOption::ImmovableObstruction
                                | ReliefOption::BunkerAbnormalOutside => "rules.ask_relief",
                                _ if hole.rules().prompt().kind == PromptKind::Choose => {
                                    "rules.choose_penalty"
                                }
                                _ => "rules.declare_unplayable",
                            };
                            (
                                call,
                                Action::Relief {
                                    option,
                                    footprint: decision.footprint,
                                },
                            )
                        }
                    };
                    let last_attempt = hole
                        .rules()
                        .last()
                        .and_then(|ruling| ruling.attempt)
                        .map_or(-1, |a| a as i64);
                    let row = rows
                        .iter()
                        .copied()
                        .find(|row| {
                            row.get("call").and_then(Variant::as_str) == Some(call)
                                && items(dict(dict(row, "input"), "rules").get("rulings").unwrap())
                                    .last()
                                    .is_some_and(|ruling| {
                                        integer(ruling.as_dict().unwrap().get("attempt").unwrap())
                                            == last_attempt
                                    })
                        })
                        .unwrap_or_else(|| panic!("{at}: missing {call} transition"));
                    hole.act(action)
                        .unwrap_or_else(|error| panic!("{at}: {error}"));
                    let Entry::Decision(actual) = hole.record().entries.last().unwrap() else {
                        panic!("{at}: missing decision");
                    };
                    if let Some(path) = reference_difference(
                        &Variant::Dict(decision.ruling),
                        &Variant::Dict(actual.ruling.clone()),
                    ) {
                        failed.push(format!("{at}.ruling.{path}"));
                    }
                    decisions += 1;
                    row
                }
            };
            let snapshot = dict(check, "output");
            let expected_rules = snapshot.get("rules").unwrap();
            if let Some(path) = reference_difference(expected_rules, &rules_value(hole.rules())) {
                failed.push(format!("{at}.rules.{path}"));
            }
            let expected_prompt = dict(snapshot, "prompt");
            let kind = PROMPTS[integer(expected_prompt.get("kind").unwrap()) as usize];
            assert_eq!(hole.rules().prompt().kind, kind, "{at}");
            let rulings = items(expected_rules.as_dict().unwrap().get("rulings").unwrap());
            let penalties: usize = rulings
                .iter()
                .map(|ruling| {
                    integer(ruling.as_dict().unwrap().get("penalty_strokes").unwrap()) as usize
                })
                .sum();
            assert_eq!(hole.rules().strokes(), rulings.len() + penalties, "{at}");
        }
        if compared {
            assert_eq!(hole.rules().prompt().kind, PromptKind::Done, "{id}");
            assert_eq!(
                hole.rules().strokes() as i64,
                integer(output.get("strokes").unwrap()),
                "{id}"
            );
        }
        assert_replays(&course, &hole);
    }
    for difference in &discrete {
        println!("Discrete difference {difference}");
    }
    println!(
        "Compared {} reference holes, {strokes} strokes and {decisions} decisions. Discrete differences: {}/{} (allowed {:.1}%).",
        cases.len(),
        discrete.len(),
        cases.len(),
        allowed_share * 100.0
    );
    println!("Replayed {} exact records byte for byte.", cases.len());
    assert!(discrete.len() as Real / cases.len() as Real <= allowed_share);
    assert!(
        failed.is_empty(),
        "Reference differences:\n{}",
        failed.join("\n")
    );
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

fn rules_value(rules: &HoleRules) -> Variant {
    object([
        ("tee", point_value(rules.tee)),
        (
            "rulings",
            Variant::Array(Array::new(rules.rulings.iter().map(ruling_value).collect())),
        ),
    ])
}

fn ruling_value(ruling: &Ruling) -> Variant {
    let point = |point: Option<Vec3>| point_value(point.unwrap_or(NO_POINT));
    let relief = ruling.relief.as_ref().map_or(Variant::Nil, |area| {
        object([
            ("option", (area.option as i64).into()),
            ("reference", point_value(area.reference)),
            ("hole", point_value(area.hole)),
            ("club_length_m", area.club_length_m.into()),
            ("bunker", outline_value(&area.bunker)),
            ("penalty_area", penalty_value(area.penalty_area.as_ref())),
            ("tee", outline_value(&area.tee)),
            ("green", outline_value(&area.green)),
            (
                "required_area",
                area.required_area.map_or(-1, |area| area as i64).into(),
            ),
            ("green_placement", Variant::Bool(area.green_placement)),
            (
                "condition",
                area.condition.as_ref().map_or(Variant::Nil, |condition| {
                    object([
                        ("name", condition.kind.name().into()),
                        ("outline", outline_value(&condition.outline)),
                    ])
                }),
            ),
            ("footprint", outline_value(&area.footprint)),
            ("ball_radius_m", area.ball_radius_m.into()),
        ])
    });
    object([
        ("status", (ruling.status as i64).into()),
        ("next", (ruling.consequence as i64).into()),
        ("penalty_strokes", (ruling.penalty_strokes as i64).into()),
        ("relief", relief),
        ("drops", (ruling.drops as i64).into()),
        ("place_at", point(ruling.place_at)),
        ("penalty_area", penalty_value(ruling.penalty_area.as_ref())),
        ("penalty_entry", point(ruling.penalty_entry)),
        (
            "playable_in_penalty_area",
            Variant::Bool(ruling.playable_in_penalty_area),
        ),
        ("embedded", Variant::Bool(ruling.embedded)),
        (
            "attempt",
            ruling.attempt.map_or(-1, |attempt| attempt as i64).into(),
        ),
        (
            "provisional",
            ruling
                .provisional
                .as_deref()
                .map_or(Variant::Nil, rules_value),
        ),
        (
            "provisional_from_last",
            Variant::Bool(ruling.provisional_from_last),
        ),
        ("from", point_value(ruling.from)),
        ("rest", point_value(ruling.rest)),
        ("spot", point(ruling.spot)),
    ])
}

fn penalty_value(area: Option<&golf_sim_rules::rulings::PenaltyAreaValue>) -> Variant {
    area.map_or(Variant::Nil, |area| {
        object([
            ("outline", outline_value(&area.outline)),
            ("red", Variant::Bool(area.color == PenaltyColor::Red)),
        ])
    })
}

fn point_value(point: Vec3) -> Variant {
    Variant::Vector3(Components {
        values: [point.x, point.y, point.z],
        width: FloatWidth::Double,
    })
}

fn outline_value(outline: &[Vec2]) -> Variant {
    Variant::PackedVector2Array(Components {
        values: outline.iter().map(|point| [point.x, point.y]).collect(),
        width: FloatWidth::Double,
    })
}

fn assert_replays(course: &std::sync::Arc<Course>, hole: &Hole) {
    let bytes = hole.record().to_bytes().unwrap();
    let replay = Hole::resume(course.clone(), &bytes).unwrap();
    assert_eq!(replay.record().to_bytes().unwrap(), bytes);
    assert_eq!(hole.rules(), replay.rules());
    assert_eq!(hole.ball_lie(), replay.ball_lie());
    assert_eq!(hole.clock_s().to_bits(), replay.clock_s().to_bits());
}

#[test]
fn skill_records_and_traces() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<Hole>();
    let course = Course::open(&testdata().join("courses/test-course")).unwrap();
    let mut records = 0;
    for profile in [Profile::SCRATCH, Profile::HCP15] {
        for seed in 1..=3 {
            let setup = HoleSetup::standard(&course, 1, Seed(seed));
            let tee = setup.tee(&course).unwrap().position;
            let mut hole = Hole::start(course.clone(), setup.clone()).unwrap();
            let mut traced = Hole::start(course.clone(), setup).unwrap().with_traces(2);
            let choice = |power| {
                let mut choice =
                    StrokeChoice::stock(ClubId::Putter, tee + Vec3::new(1.0, 0.0, 0.0))
                        .with_aim_direction(Vec3::new(1.0, 0.0, 0.0))
                        .with_swing(Swing::Skill(profile));
                choice.address = choice.address.with_value(Setting::SwingLength, power);
                Action::Stroke(choice)
            };
            let mut history = Vec::new();
            for power in [0.2, 0.3, 0.4] {
                hole.act(choice(power)).unwrap();
                traced.act(choice(power)).unwrap();
                history.push(traced.traces().back().unwrap().clone());
            }
            assert!(hole.traces().is_empty());
            assert!(traced.traces().iter().eq(&history[1..]));
            for trace in traced.traces() {
                assert!(
                    trace
                        .phases()
                        .iter()
                        .any(|phase| phase.phase == Phase::Delivery)
                );
                assert_eq!(trace.phases().last().unwrap().phase, Phase::Ruling);
            }
            assert_eq!(
                hole.record().to_bytes().unwrap(),
                traced.record().to_bytes().unwrap()
            );
            assert_replays(&course, &traced);
            let bytes = traced.record().to_bytes().unwrap();
            let mut replay = Hole::resume(course.clone(), &bytes).unwrap();
            replay.act(choice(0.25)).unwrap();
            hole.act(choice(0.25)).unwrap();
            assert_eq!(
                replay.record().to_bytes().unwrap(),
                hole.record().to_bytes().unwrap()
            );
            records += 1;
        }
    }
    println!(
        "Replayed {records} skill records byte for byte with identical records with tracing enabled."
    );
}

#[test]
fn replay_names_the_first_changed_result() {
    let course = Course::open(&testdata().join("courses/test-course")).unwrap();
    let setup = HoleSetup::standard(&course, 1, Seed(7));
    let aim = setup.cup(&course).unwrap().position;
    let mut hole = Hole::start(course.clone(), setup).unwrap();
    assert!(hole.act(Action::GoForward).is_err());
    assert!(hole.record().entries.is_empty());
    hole.act(Action::Stroke(
        StrokeChoice::stock(ClubId::Driver, aim).with_swing(Swing::Exact {
            power: 0.5,
            timing_fault: 0.0,
            strike_m: None,
        }),
    ))
    .unwrap();
    let mut record = hole.record().clone();
    let Entry::Stroke(stroke) = &mut record.entries[0] else {
        panic!("expected stroke")
    };
    assert_eq!(stroke.address_usec, Some(250_000));
    assert_eq!(
        stroke.facts.gust_time_s.to_bits(),
        (stroke.facts.execution.impact_usec as Real / 1_000_000.0).to_bits()
    );
    stroke.facts.lie.ball_height_m += 0.001;
    let assert_diverges = |bytes: &[u8], field| {
        assert!(matches!(
            Hole::resume(course.clone(), bytes),
            Err(Refusal::Invalid(Error::Diverged { entry: 0, path })) if path == field
        ));
    };
    let bytes = record.to_bytes().unwrap();
    assert_diverges(&bytes, "lie.ball_height_m");
    let Entry::Stroke(stroke) = &mut record.entries[0] else {
        panic!("expected stroke")
    };
    stroke.facts.execution.timing_fault = -0.0;
    assert_diverges(&record.to_bytes().unwrap(), "execution.timing_fault");
    hole.act(Action::Relief {
        option: ReliefOption::StrokeAndDistance,
        footprint: vec![],
    })
    .unwrap();
    let mut record = hole.record().clone();
    let Entry::Decision(decision) = &mut record.entries[1] else {
        panic!("expected decision")
    };
    *decision.ruling.get_mut("penalty").unwrap() = 4_i64.into();
    assert!(matches!(
        Hole::resume(course, &record.to_bytes().unwrap()),
        Err(Refusal::Invalid(Error::Diverged { entry: 1, path })) if path == "ruling.penalty"
    ));
}

#[test]
fn replay_judges_stamped_presses_again() {
    let course = Course::open(&testdata().join("courses/test-course")).unwrap();
    let mut setup = HoleSetup::standard(&course, 1, Seed(7));
    setup.settings.strike_variant = StrikeVariant::B;
    let mut hole = Hole::start(course.clone(), setup.clone()).unwrap();
    let ball = hole.ball_lie().unwrap();
    let aim = Vec3::new(1.0, 0.0, 0.0);
    let club = ClubId::Driver;
    let address = Address::stock(club);
    let seed = setup.seed.derive("attempt/0");
    let world = setup.world(&course, hole.clock_s()).unwrap();
    let input = stroke::StrokeInput::new(
        DeliveryInput {
            club,
            lie: world.address_lie(ball, &address.footprint(club), aim),
            address,
            attempt_seed: seed,
        },
        aim,
    );
    let spec = stroke::swing_spec(&input, setup.settings.assists());
    let address_usec = 1_000_000;
    let start = address_usec + 250_000;
    let power = start + 550_000;
    let mut session = SwingSession::new(spec, start, stroke::reticle_seed(seed));
    session.press(power);
    let impact = session.close_usec();
    session.press(impact);
    let execution = session.execution().unwrap();
    hole.act(Action::Stroke(
        StrokeChoice::stock(club, ball.spot + aim).with_swing(Swing::Exact {
            power: execution.power,
            timing_fault: execution.timing_fault,
            strike_m: execution.strike_m,
        }),
    ))
    .unwrap();
    let mut record = hole.record().clone();
    record.driver = HoleRecord::console_driver();
    let Entry::Stroke(stroke) = &mut record.entries[0] else {
        panic!("expected stroke")
    };
    stroke.address_usec = Some(address_usec);
    stroke.facts.execution = execution;
    stroke.swing = StrokeEntry::pressed_swing([start, power, impact].map(SwingPress::Press));
    let bytes = record.to_bytes().unwrap();
    let replay = Hole::resume(course, &bytes).unwrap();
    assert_eq!(replay.record().to_bytes().unwrap(), bytes);
}
