use std::{collections::BTreeMap, path::Path};

use golf_sim::{
    action::{Action, ReliefOption, StrokeChoice, Swing},
    hole::{Hole, prompt::ActionRefusal},
    records::hole::DecisionKind,
    setup::{HoleSetup, Mode},
};
use golf_sim_base::{
    ClubId, PromptKind, Real, Refusal, Seed, Vec3, corpus,
    variant::{Dict, Variant},
};
use golf_sim_course::course::{Course, Tee};
use golf_sim_stroke::address::Setting;
use golf_sim_testkit::reference_records::{ReferenceEntry, decode_hole};

const RELIEF_OPTIONS: [ReliefOption; 11] = {
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
const HOLE_POSITION_STEPS: Real = 8.0 * f32::EPSILON as Real;
const STROKE_POSITION_M: Real = 0.05;
const STROKE_CARRY_M: Real = 0.1;

#[test]
fn preview_uses_the_address_and_target() {
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata");
    let course = Course::open(&data.join("courses/test-course")).unwrap();
    let setup = HoleSetup::standard(&course, 1, Seed(1));
    let hole = Hole::start(course, setup).unwrap();
    let ball = hole.ball_lie().unwrap();
    let record = hole.record().to_bytes().unwrap();
    let target = ball.spot + Vec3::new(40.0, 0.0, -30.0);
    let choice = StrokeChoice::stock(ClubId::Iron7, target);
    let preview = hole.preview(&choice).unwrap();
    assert!(preview.carry_m.is_finite());
    assert_eq!(preview.target_distance_m, 50.0);

    let mut choice = choice.with_swing(Swing::Exact {
        power: 0.4,
        timing_fault: 0.7,
        strike_m: None,
    });
    let same_preview = hole.preview(&choice).unwrap();
    assert_eq!(preview.carry_m, same_preview.carry_m);
    assert_eq!(preview.carry_to_target_m, same_preview.carry_to_target_m);

    choice.address = choice.address.with_value(Setting::SwingLength, 0.6);
    let shorter = hole.preview(&choice).unwrap();
    assert!(shorter.carry_m < preview.carry_m);
    assert!(shorter.has_hint);
    assert_eq!(hole.ball_lie(), Some(ball));
    assert_eq!(hole.record().to_bytes().unwrap(), record);
}

#[test]
fn reference_advice_and_actions() {
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata");
    let course = Course::open(&data.join("courses/test-course")).unwrap();
    let advice = corpus::read(&data.join("hole/advice/fixture.bin")).unwrap();
    let mut by_record = BTreeMap::<_, Vec<_>>::new();
    for case in cases(&advice) {
        by_record
            .entry(id(case).rsplit_once("/address/").unwrap().0)
            .or_default()
            .push(case);
    }
    for addresses in by_record.values_mut() {
        addresses.sort_by_key(|case| id(case));
    }
    let records = corpus::read(&data.join("records/holes/fixture.bin")).unwrap();
    assert_eq!(cases(&records).count(), 30);
    let mut addresses_checked = [0; 2];
    let mut states_checked = 0;
    let mut actions_checked = 0;
    let mut near_threshold = 0;
    for record in cases(&records) {
        let record_id = id(record);
        let addresses = by_record.remove(record_id).unwrap();
        let Variant::PackedByteArray(bytes) = get(dict(record, "output"), "bytes") else {
            panic!("{record_id}: expected record bytes");
        };
        let reference = decode_hole(bytes, &course.data().id).unwrap();
        let mode = reference.setup.settings.info_mode;
        let cup = reference.setup.cup(&course).unwrap().position;
        let tee = reference.setup.tee(&course).unwrap();
        let mut hole = Hole::start(course.clone(), reference.setup).unwrap();
        let mut next_address = addresses.into_iter();
        let mut attempt = 0;
        for (index, entry) in reference
            .entries
            .into_iter()
            .map(Some)
            .chain([None])
            .enumerate()
        {
            let at = format!("{record_id}/prefix/{index}");
            let actions = hole.legal();
            let bytes = hole.record().to_bytes().unwrap();
            assert_eq!(hole.prompt(), hole.rules().prompt(), "{at}");
            assert_eq!(
                hole.prompt().kind == PromptKind::Done,
                actions.is_empty(),
                "{at}"
            );
            for action in &actions {
                let mut trial = Hole::resume(course.clone(), &bytes).unwrap();
                trial
                    .act(action.clone())
                    .unwrap_or_else(|error| panic!("{at}: offered {action:?}: {error}"));
                actions_checked += 1;
            }
            let outside = if hole.prompt().kind == PromptKind::MaybeLost {
                Action::Place(Vec3::ZERO)
            } else {
                Action::GoForward
            };
            assert!(!actions.contains(&outside), "{at}");
            assert_eq!(
                hole.act(outside).unwrap_err(),
                ActionRefusal::Rules(Refusal::NotNow {
                    prompt: hole.prompt().kind
                }),
                "{at}"
            );
            assert_eq!(hole.record().to_bytes().unwrap(), bytes, "{at}");
            states_checked += 1;

            if hole.prompt().kind == PromptKind::Play {
                let case = next_address
                    .next()
                    .unwrap_or_else(|| panic!("{at}: missing address"));
                let input = dict(case, "input");
                assert_eq!(integer(input, "attempt"), attempt, "{at}: {}", id(case));
                let delta = hole.ball_lie().unwrap().spot - vector(input, "ball_position");
                assert!(
                    delta.length() <= STROKE_POSITION_M,
                    "{at}: {} spot differs by {delta:?}",
                    id(case)
                );
                near_threshold += check_address(&hole, case, mode, cup, tee);
                assert_eq!(hole.record().to_bytes().unwrap(), bytes, "{}", id(case));
                addresses_checked[mode as usize] += 1;
            } else {
                assert_eq!(hole.advice(), None, "{at}");
            }

            if let Some(entry) = entry {
                let action = replay_action(entry);
                assert!(
                    actions.iter().any(|offered| same_option(offered, &action)),
                    "{at}: recorded action {action:?} missing from {actions:?}"
                );
                attempt += i64::from(matches!(action, Action::Stroke(_)));
                hole.act(action)
                    .unwrap_or_else(|error| panic!("{at}: {error}"));
            }
        }
        if let Some(case) = next_address.next() {
            panic!("unmatched address {}", id(case));
        }
    }
    assert!(by_record.is_empty());
    let [caddie, tour] = addresses_checked;
    assert_eq!(caddie + tour, 127);
    println!(
        "Compared 127 reference addresses. Listed {near_threshold} carry thresholds within stroke tolerance."
    );
    println!("Accepted {caddie} Caddie previews and refused {tour} Tour previews.");
    println!(
        "Checked {actions_checked} offered actions and {states_checked} typed refusals across 30 reference records."
    );
}

fn same_option(offered: &Action, played: &Action) -> bool {
    match (offered, played) {
        (Action::Stroke(_), Action::Stroke(_)) | (Action::Drop(_), Action::Drop(_)) => true,
        (Action::Relief { option: left, .. }, Action::Relief { option: right, .. }) => {
            left == right
        }
        _ => offered == played,
    }
}

fn check_address(hole: &Hole, case: &Dict, mode: Mode, cup: Vec3, tee: &Tee) -> usize {
    let id = id(case);
    let result = hole.advice().unwrap();
    let expected = dict(case, "output");
    let club = ClubId::from_name(get(expected, "club_id").as_str().unwrap()).unwrap();
    check_first_aim(hole, case, result.first_aim, cup, tee);
    let choice = |club| StrokeChoice::stock(club, cup).with_aim_direction(result.first_aim);
    if mode == Mode::Tour {
        assert_eq!(
            hole.preview(&choice(result.club)).unwrap_err(),
            ActionRefusal::PreviewUnavailable,
            "{id}"
        );
        assert_eq!(result.club, club, "{id}");
        return 0;
    }
    let preview = hole.preview(&choice(result.club)).unwrap();
    assert!(
        preview.carry_m.is_finite() || preview.roll_m.is_finite(),
        "{id}"
    );

    let needed = 0.95 * flat(cup - hole.ball_lie().unwrap().spot).length();
    let clubs = &ClubId::ALL[ClubId::Wood3 as usize..=ClubId::LobWedge as usize];
    let carry = |club| hole.preview(&choice(club)).unwrap().carry_to_target_m;
    let mut near_threshold = 0;
    let mut neighbouring_club = false;
    if !hole.prompt().from_last_spot() && needed >= 0.95 * 35.0 && clubs.contains(&result.club) {
        for (index, &threshold_club) in clubs.iter().enumerate() {
            let threshold_carry = carry(threshold_club);
            if (threshold_carry - needed).abs() <= STROKE_CARRY_M {
                let neighbour = if index == 0 {
                    clubs[1]
                } else {
                    clubs[index - 1]
                };
                let neighbour_carry = carry(neighbour);
                println!(
                    "{id}: wanted {needed:.6} m, {} {threshold_carry:.6} m, {} {neighbour_carry:.6} m",
                    threshold_club.name(),
                    neighbour.name()
                );
                near_threshold += 1;
                let pair = [threshold_club, neighbour];
                neighbouring_club |= index > 0
                    && neighbour_carry >= needed - STROKE_CARRY_M
                    && pair.contains(&club)
                    && pair.contains(&result.club);
            }
        }
    }
    assert!(
        result.club == club || neighbouring_club,
        "{id}: expected {club:?}, got {:?}, wanted {needed:.6} m, carries {:.6} m and {:.6} m",
        result.club,
        carry(club),
        carry(result.club)
    );
    near_threshold
}

fn check_first_aim(hole: &Hole, case: &Dict, actual: Vec3, cup: Vec3, tee: &Tee) {
    let reference = vector(dict(case, "output"), "first_aim");
    let reference_spot = vector(dict(case, "input"), "ball_position");
    let ball = hole.ball_lie().unwrap();
    let id = id(case);
    assert!(
        (actual.length() - 1.0).abs() <= HOLE_POSITION_STEPS && actual.y == 0.0,
        "{id}: aim must be a horizontal unit vector"
    );
    if ball.is_teed() && (ball.spot - tee.position).length() < 5.0 {
        assert_position(id, actual, reference);
        return;
    }

    // Replay can move the origin by the stroke tolerance. Compare where each
    // bearing points at the target's range, from its own ball position.
    let mut target = cup;
    if flat(cup - reference_spot).length() > 230.0 {
        let direction_error =
            |point| (flat(point - reference_spot).normalized() - reference).length_squared();
        target = *tee
            .play_line
            .iter()
            .min_by(|&&left, &&right| direction_error(left).total_cmp(&direction_error(right)))
            .unwrap();
    }
    let aim_point = |spot, direction| flat(spot) + direction * flat(target - spot).length();
    assert_position(
        id,
        aim_point(ball.spot, actual),
        aim_point(reference_spot, reference),
    );
}

fn assert_position(id: &str, actual: Vec3, reference: Vec3) {
    let magnitude = |vector: Vec3| vector.x.abs().max(vector.y.abs()).max(vector.z.abs());
    let tolerance = HOLE_POSITION_STEPS * magnitude(actual).max(magnitude(reference)).max(1.0);
    assert!(
        magnitude(actual - reference) <= tolerance,
        "{id}: aim {actual:?} vs {reference:?}, tolerance {tolerance:.9}"
    );
}

fn flat(point: Vec3) -> Vec3 {
    Vec3::new(point.x, 0.0, point.z)
}

fn replay_action(entry: ReferenceEntry) -> Action {
    match entry {
        ReferenceEntry::Stroke(facts) => {
            let mut choice = StrokeChoice::stock(facts.club_id, facts.aim)
                .with_aim_direction(facts.aim)
                .with_swing(Swing::Exact {
                    power: facts.execution.power,
                    timing_fault: facts.execution.timing_fault,
                    strike_m: facts.execution.strike_m,
                });
            choice.address = facts.address;
            choice.flagstick_in = Some(facts.flagstick_in);
            Action::Stroke(choice)
        }
        ReferenceEntry::Decision(decision) => match decision.kind {
            DecisionKind::GoForward => Action::GoForward,
            DecisionKind::Drop => Action::Drop(decision.point),
            DecisionKind::Place => Action::Place(decision.point),
            DecisionKind::Relief => Action::Relief {
                option: RELIEF_OPTIONS[decision.option as usize],
                footprint: decision.footprint,
            },
        },
    }
}

fn cases(fixture: &Variant) -> impl Iterator<Item = &Dict> {
    let Variant::Array(cases) = get(fixture.as_dict().unwrap(), "cases") else {
        panic!("expected cases");
    };
    cases.items.iter().map(|case| case.as_dict().unwrap())
}

fn id(case: &Dict) -> &str {
    get(case, "id").as_str().unwrap()
}

fn get<'a>(dict: &'a Dict, key: &str) -> &'a Variant {
    dict.get(key).unwrap_or_else(|| panic!("missing {key}"))
}

fn dict<'a>(fields: &'a Dict, key: &str) -> &'a Dict {
    get(fields, key).as_dict().unwrap()
}

fn integer(dict: &Dict, key: &str) -> i64 {
    let Variant::Int(number) = get(dict, key) else {
        panic!("{key}: expected integer")
    };
    number.value()
}

fn vector(dict: &Dict, key: &str) -> Vec3 {
    let Variant::Vector3(vector) = get(dict, key) else {
        panic!("{key}: expected vector")
    };
    let [x, y, z] = vector.values;
    Vec3::new(x, y, z)
}
