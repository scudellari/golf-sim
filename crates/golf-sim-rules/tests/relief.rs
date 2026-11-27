use std::path::Path;

use golf_sim_base::{
    PromptKind, Vec3,
    trace::{NoProbe, Phase, Trace},
};
use golf_sim_rules::{
    relief::{self, AbnormalKind, ReliefReason},
    rulings::RulesRefusal,
};
use golf_sim_stroke::results::DropResult;
use golf_sim_testkit::corpus::{FixtureRun, rules::outside_bunker_case};

#[test]
fn relief_matches_reference() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/rules/relief/fixture.bin");
    let report = FixtureRun::read(&path, Some("rules"), &[])
        .unwrap()
        .check()
        .unwrap();
    assert!(report.checked > 0);
    assert!(report.discrete.is_empty(), "{report}");
    assert!(report.skipped.len() >= 2);
    print!("{report}");
}

#[test]
fn outside_bunker_reason_survives_drop_retries_and_placement() {
    let testdata = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata");
    let (rules, area, course, first) = outside_bunker_case(&testdata);
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
