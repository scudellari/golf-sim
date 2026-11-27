use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use golf_sim_base::{
    PromptKind, Real, Vec2, Vec3, corpus,
    digest::object,
    trace::{NoProbe, Phase, Trace},
    variant::{self, Dict, Variant},
};
use golf_sim_rules::{
    relief::{self, AbnormalCondition, AbnormalKind, ReliefOption, ReliefReason},
    rulings::RulesRefusal,
};
use golf_sim_stroke::results::DropResult;
use golf_sim_testkit::tolerance;

#[path = "transitions.rs"]
mod transitions_fixture;

use transitions_fixture::{
    NO_POINT, checked_replay, condition_value, dict, double, field, integer, items, legal_course,
    outline, outline_value, point_value, point3, read_condition, read_kind, read_relief,
    read_rules, real, replay,
};

#[test]
fn relief_matches_reference() {
    let testdata = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata");
    let Variant::Dict(mut fixture) =
        corpus::read(&testdata.join("rules/relief/fixture.bin")).unwrap()
    else {
        unreachable!();
    };
    let Variant::Array(cases) = fixture.get_mut("cases").unwrap() else {
        unreachable!();
    };
    assert_eq!(cases.items.len(), 2738);

    let mut world_cases = 0;
    let mut stroke_cases = 0;
    cases.items.retain_mut(|case| {
        let Variant::Dict(case) = case else {
            unreachable!();
        };
        let call = field(case, "call").as_str().unwrap();
        match call {
            "relief_ground.conditions"
            | "relief_ground.tee_outline"
            | "relief_ground.drop_spots"
            | "relief_ground._cart_path_outlines" => {
                world_cases += 1;
                false
            }
            "stroke.drop" | "stroke.place" => {
                stroke_cases += 1;
                false
            }
            "complete_relief.nearest"
            | "abnormal_condition.closest_edge"
            | "relief_area.outline"
            | "relief_area.flat" => {
                let expected = case.get_mut("output").unwrap();
                *expected = object([("position", std::mem::replace(expected, Variant::Nil))]);
                true
            }
            _ => true,
        }
    });
    assert_eq!(world_cases, 23);
    assert_eq!(stroke_cases, 15);
    assert_eq!(cases.items.len(), 2700);

    // The recorder groups these calls under rules/relief. Select the relief tolerance row.
    *fixture.get_mut("layer").unwrap() = "relief".into();
    let scratch = Scratch::new(&testdata);
    let path = scratch.0.join("testdata/fixture.bin");
    let body = variant::encode(&Variant::Dict(fixture)).unwrap();
    let mut bytes = b"GSCORP01".to_vec();
    bytes.extend(u32::try_from(body.len()).unwrap().to_le_bytes());
    bytes.extend(body);
    fs::write(&path, bytes).unwrap();

    let report = tolerance::check(&path, "relief", |case| {
        let call = field(case, "call").as_str().unwrap();
        let input = dict(field(case, "input"));
        checked_replay(case, run(call, input))
    })
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(report.checked, 2700);
    assert!(report.discrete.is_empty());
    assert!(report.skipped.is_empty());
    print!("{report}");
    println!("All 2700 relief cases matched, including 14 drop and placement rulings.");
}

#[test]
fn outside_bunker_reason_survives_drop_retries_and_placement() {
    let testdata = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata");
    let fixture = corpus::read(&testdata.join("rules/transitions/fixture.bin")).unwrap();
    let inputs = |call: &'static str| {
        items(field(dict(&fixture), "cases"))
            .iter()
            .map(dict)
            .filter(move |case| field(case, "call").as_str() == Some(call))
            .map(|case| dict(field(case, "input")))
    };
    let (input, area) = inputs("rules.ask_relief")
        .map(|input| (input, read_relief(field(input, "area"))))
        .find(|(_, area)| area.option == ReliefOption::BunkerAbnormalOutside)
        .unwrap();
    let first = inputs("rules.after_drop")
        .find(|input| {
            read_rules(field(input, "rules"))
                .last()
                .and_then(|last| last.relief.as_ref())
                .is_some_and(|area| area.option == ReliefOption::BunkerAbnormalOutside)
        })
        .map(|input| point3(field(dict(field(input, "drop")), "first")))
        .unwrap();
    let rules = read_rules(field(input, "rules"));
    let course = legal_course(dict(field(input, "legal")));
    let legal = course.legal();
    let mut trace = Trace::default();

    let pending = relief::ask_relief(&rules, area.clone(), &legal, &mut trace);
    assert_eq!(
        pending,
        relief::ask_relief(&rules, area, &legal, &mut NoProbe)
    );
    let reason = ReliefReason::AbnormalCondition {
        kind: AbnormalKind::TemporaryWater,
        outside_bunker: true,
    };
    assert_eq!(pending.prompt().kind, PromptKind::Drop);
    assert_eq!(pending.rules.last().unwrap().reason, Some(reason));
    assert_eq!(pending.rules.penalties(), 0);

    let bad_drop = DropResult {
        first: None,
        rest: None,
    };
    let refused = relief::after_drop(&pending.rules, &bad_drop, &legal, &mut trace);
    assert_eq!(
        refused.refusal,
        Some(RulesRefusal::DropFirstOutsideReliefArea)
    );
    assert_eq!(refused.rules, pending.rules);
    assert_eq!(trace.phases().len(), 1);

    let rolled_out = DropResult {
        first: Some(first),
        rest: None,
    };
    let retry = relief::after_drop(&pending.rules, &rolled_out, &legal, &mut trace);
    assert_eq!(retry.prompt().kind, PromptKind::Drop);
    assert_eq!(retry.rules.last().unwrap().reason, Some(reason));
    let placing = relief::after_drop(&retry.rules, &rolled_out, &legal, &mut trace);
    assert_eq!(placing.prompt().kind, PromptKind::Place);
    assert_eq!(placing.rules.last().unwrap().place_at, Some(first));
    assert_eq!(placing.rules.last().unwrap().reason, Some(reason));
    assert_eq!(placing.rules.penalties(), 0);

    let misplaced = relief::place(
        &placing.rules,
        first + Vec3::new(0.0, 0.001, 0.0),
        &mut trace,
    );
    assert_eq!(misplaced.refusal, Some(RulesRefusal::WrongPlacementSpot));
    let placed = relief::place(&placing.rules, first, &mut trace);
    assert_eq!(placed.prompt().kind, PromptKind::Play);
    assert_eq!(placed.rules.last().unwrap().reason, Some(reason));
    assert_eq!(placed.rules.ball(), Some(first));
    assert_eq!(placed.rules.penalties(), 1);

    assert_eq!(trace.phases().len(), 4);
    for phase in trace.phases() {
        assert_eq!(phase.phase, Phase::Ruling);
        assert_eq!(phase.values[0].name, "Rule 16.1c(2) – Temporary water");
    }
    assert_eq!(trace.phases()[3].values[0].value, 1.0);
}

fn run(call: &str, input: &Dict) -> Variant {
    let get = |key| field(input, key);
    let area = || read_relief(get("self"));
    let condition = || read_condition(get("self"));
    let course = || legal_course(dict(get("legal")));
    let point = || point3(get("point"));
    let plan = || point2(get("point"));
    let radius = || real(get("radius"));
    let position = |value| object([("position", value)]);
    match call {
        "relief_area.flat" => position(vector2(relief::flat(point()))),
        "relief_area.back" => vector2(area().back()),
        "relief_area.on_line" => Variant::Bool(area().on_line()),
        "relief_area.penalty" => (area().penalty() as i64).into(),
        "relief_area.reach_m" => area().reach_m().into(),
        "relief_area.outline" => {
            let points = usize::try_from(integer(get("points"))).unwrap();
            position(outline_value(&area().outline(points)))
        }
        "relief_area.accepts_first" => {
            Variant::Bool(area().accepts_first(point(), &course().legal()))
        }
        "relief_area.accepts_rest" => {
            Variant::Bool(area().accepts_rest(point(), point3(get("first")), &course().legal()))
        }
        "relief_area.allows" => Variant::Bool(area().allows(point(), &course().legal())),
        "abnormal_condition.create" => condition_value(&AbnormalCondition::new(
            read_kind(get("label")),
            outline(get("polygon")),
        )),
        "abnormal_condition.touches" => Variant::Bool(condition().touches(plan(), radius())),
        "abnormal_condition.interferes" => {
            let end = point2(get("line_to"));
            Variant::Bool(condition().interferes(
                plan(),
                &outline(get("offsets")),
                radius(),
                (end.x.is_finite() && end.y.is_finite()).then_some(end),
            ))
        }
        "abnormal_condition.closest_edge" => {
            let closest = relief::closest_edge(plan(), &outline(get("polygon")))
                .unwrap_or(Vec2::new(Real::INFINITY, Real::INFINITY));
            position(vector2(closest))
        }
        "complete_relief.extent_limit" => relief::extent_limit(
            &outline(get("outline")),
            point2(get("origin")),
            point2(get("offset")),
        )
        .into(),
        "complete_relief.nearest" => {
            let spot = relief::nearest(
                point3(get("origin")),
                &read_relief(get("area")),
                &course().legal(),
            );
            position(point_value(spot.unwrap_or(NO_POINT)))
        }
        "rules.after_drop" | "rules.place" => replay(call, input),
        _ => panic!("unexpected call {call}"),
    }
}

fn point2(value: &Variant) -> Vec2 {
    let Variant::Vector2(point) = value else {
        panic!("expected a Vector2");
    };
    let [x, y] = point.values;
    Vec2::new(x, y)
}

fn vector2(point: Vec2) -> Variant {
    Variant::Vector2(double([point.x, point.y]))
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(testdata: &Path) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("golf-relief-{}-{now}", std::process::id()));
        let scratch_testdata = path.join("testdata");
        fs::create_dir_all(&scratch_testdata).unwrap();
        for name in ["tolerances.toml", "departures.toml"] {
            fs::copy(testdata.join(name), scratch_testdata.join(name)).unwrap();
        }
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
