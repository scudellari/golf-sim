use std::{collections::BTreeSet, path::Path};

use golf_sim_base::{PromptKind, Surface, Vec3, variant::Dict};
use golf_sim_course::legal::Area;
use golf_sim_rules::{rulings::RulesRefusal, transitions};
use golf_sim_stroke::results::{End, StrokeOutcome};
use golf_sim_testkit::corpus::{FixtureRun, rules::synthetic_course};

const RULES_CALLS: [&str; 8] = [
    "rules.start_hole",
    "rules.after_stroke",
    "rules.status",
    "rules.consequence",
    "rules.go_forward",
    "rules.provisional_available",
    "rules.declare_provisional",
    "rules.penalty_options",
];

const RELIEF_CALLS: [&str; 9] = [
    "rules.relief_available",
    "rules.ask_relief",
    "rules.unplayable_options",
    "rules.declare_unplayable",
    "rules.embedded_available",
    "rules.embedded_relief",
    "rules.after_drop",
    "rules.place",
    "rules.choose_penalty",
];

#[test]
fn transitions_match_reference() {
    let testdata = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata");
    let path = testdata.join("rules/transitions/fixture.bin");
    let mut calls = BTreeSet::new();
    let mut relief_cases = 0;
    let fixture = FixtureRun::read(&path, Some("rules"), &[]).unwrap();
    let case_count = fixture.cases().len();
    let is_relief =
        |case: &Dict| RELIEF_CALLS.contains(&case.get("call").unwrap().as_str().unwrap());
    let expected_relief = fixture.cases().filter(|case| is_relief(case)).count();
    let report = fixture
        .check_observing(|case| {
            let call = case.get("call").unwrap().as_str().unwrap();
            calls.insert(call.to_owned());
            relief_cases += usize::from(is_relief(case));
        })
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        calls,
        RULES_CALLS
            .into_iter()
            .chain(RELIEF_CALLS)
            .map(str::to_owned)
            .collect()
    );
    assert_eq!(report.checked, case_count);
    assert_eq!(relief_cases, expected_relief);
    assert!(report.discrete.is_empty());
    assert!(report.skipped.is_empty());
    print!("{report}");
    println!(
        "All {} transition cases matched the rules tolerance, including {relief_cases} relief transitions.",
        report.checked
    );
}

#[test]
fn pick_up_ends_play_at_the_current_count() {
    let tee = Vec3::ZERO;
    let start = transitions::start_hole(tee);
    let empty = transitions::pick_up(&start.rules);
    assert_eq!(empty.refusal, None);
    assert_eq!(empty.prompt().kind, PromptKind::Done);
    assert_eq!(empty.rules.strokes(), 0);

    let legal = synthetic_course(Vec::new(), Vec::new(), Vec::new());
    let oob = StrokeOutcome {
        end: End::AtRest,
        start: tee,
        rest: Vec3::new(1000.0, 0.0, 1000.0),
        rest_surface: Surface::Fairway,
        ball_radius_m: 0.021335,
        carry_m: 0.0,
        total_m: 0.0,
        embedded: false,
        penalty_entry: None,
        marks: Vec::new(),
    };
    let legal = legal.legal();
    let lost = transitions::after_stroke(&start.rules, &oob, &legal, None, None, false);
    let replay = transitions::go_forward(&lost.rules, None, Area::General);
    let pending = transitions::after_stroke(&replay.rules, &oob, &legal, None, None, false);
    assert_eq!(pending.prompt().kind, PromptKind::MaybeLost);

    let done = transitions::pick_up(&pending.rules);
    assert_eq!(done.refusal, None);
    assert_eq!(done.prompt().kind, PromptKind::Done);
    assert!(done.rules.picked_up());
    assert!(!done.rules.holed());
    assert_eq!(done.rules.ball(), None);
    assert_eq!(done.rules.strokes(), 3);
    assert_eq!(done.rules.penalties(), 1);
    assert_eq!(done.rules.rulings[..2], pending.rules.rulings);
    assert_eq!(pending.rules.prompt().kind, PromptKind::MaybeLost);

    let refused = transitions::pick_up(&done.rules);
    assert_eq!(refused.refusal, Some(RulesRefusal::HoleFinished));
    assert_eq!(refused.rules, done.rules);
    let refused = transitions::after_stroke(&done.rules, &oob, &legal, None, None, false);
    assert_eq!(
        refused.refusal,
        Some(RulesRefusal::StrokeWhileFinishedOrPending)
    );
    assert_eq!(refused.rules, done.rules);
}
