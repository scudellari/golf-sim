use golf_sim::{
    action::{Action, StrokeChoice},
    hole::{Hole, prompt::ActionRefusal},
    setup::Mode,
};
use golf_sim_base::{ClubId, PromptKind, Real, Refusal, Vec3, digest::object, variant::Dict};
use golf_sim_course::course::{Course, Tee};

use super::{Pair, dict, get, id, vector};
use crate::corpus::rules::point_value;

const HOLE_POSITION_STEPS: Real = 8.0 * f32::EPSILON as Real;
const STROKE_POSITION_M: Real = 0.05;
const STROKE_CARRY_M: Real = 0.1;

fn same_option(offered: &Action, played: &Action) -> bool {
    match (offered, played) {
        (Action::Stroke(_), Action::Stroke(_)) | (Action::Drop(_), Action::Drop(_)) => true,
        (Action::Relief { option: left, .. }, Action::Relief { option: right, .. }) => {
            left == right
        }
        _ => offered == played,
    }
}

fn check_address(hole: &Hole, case: &Dict, mode: Mode, cup: Vec3, tee: &Tee, drift: bool) -> Pair {
    let id = id(case);
    let result = hole.advice().unwrap();
    let expected = dict(case, "output");
    let club = ClubId::from_name(get(expected, "club_id").as_str().unwrap()).unwrap();
    let (reference_aim, actual_aim) = first_aim(hole, case, result.first_aim, cup, tee);
    if !drift {
        assert_position(id, actual_aim, reference_aim);
    }
    let snapshot = |club: ClubId, aim| {
        object([
            ("club_id", club.name().into()),
            ("first_aim", point_value(aim)),
        ])
    };
    let choice = |club| StrokeChoice::stock(club, cup).with_aim_direction(result.first_aim);
    if mode == Mode::Tour {
        assert_eq!(
            hole.preview(&choice(result.club)).unwrap_err(),
            ActionRefusal::PreviewUnavailable,
            "{id}"
        );
        return (
            snapshot(club, reference_aim),
            snapshot(result.club, actual_aim),
        );
    }
    let preview = hole.preview(&choice(result.club)).unwrap();
    assert!(
        preview.carry_m.is_finite() || preview.roll_m.is_finite(),
        "{id}"
    );

    let needed = 0.95 * flat(cup - hole.ball_lie().unwrap().spot).length();
    let clubs = &ClubId::ALL[ClubId::Wood3 as usize..=ClubId::LobWedge as usize];
    let carry = |club| hole.preview(&choice(club)).unwrap().carry_to_target_m;
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
                let pair = [threshold_club, neighbour];
                neighbouring_club |= index > 0
                    && neighbour_carry >= needed - STROKE_CARRY_M
                    && pair.contains(&club)
                    && pair.contains(&result.club);
            }
        }
    }
    let actual_club = if neighbouring_club && !drift {
        club
    } else {
        result.club
    };
    (
        snapshot(club, reference_aim),
        snapshot(actual_club, actual_aim),
    )
}

fn first_aim(hole: &Hole, case: &Dict, actual: Vec3, cup: Vec3, tee: &Tee) -> (Vec3, Vec3) {
    let reference = vector(dict(case, "output"), "first_aim");
    let reference_spot = vector(dict(case, "input"), "ball_position");
    let ball = hole.ball_lie().unwrap();
    let id = id(case);
    assert!(
        (actual.length() - 1.0).abs() <= HOLE_POSITION_STEPS && actual.y == 0.0,
        "{id}: aim must be a horizontal unit vector"
    );
    if ball.is_teed() && (ball.spot - tee.position).length() < 5.0 {
        return (reference, actual);
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
    (
        aim_point(reference_spot, reference),
        aim_point(ball.spot, actual),
    )
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

pub(super) fn run(
    hole: &Hole,
    course: &Course,
    case: &Dict,
    mode: Mode,
    drift: bool,
) -> super::Pair {
    let setup = &hole.record().setup;
    let cup = setup.cup(course).unwrap().position;
    let tee = setup.tee(course).unwrap();
    let bytes = hole.record().to_bytes().unwrap();
    let delta = hole.ball_lie().unwrap().spot - vector(dict(case, "input"), "ball_position");
    assert!(
        delta.length() <= STROKE_POSITION_M || drift,
        "{} spot differs by {delta:?}",
        id(case)
    );
    let pair = check_address(hole, case, mode, cup, tee, drift);
    assert_eq!(hole.record().to_bytes().unwrap(), bytes, "{}", id(case));
    pair
}

pub(super) fn check_actions(
    hole: &mut Hole,
    course: &std::sync::Arc<Course>,
    action: Option<&Action>,
    at: &str,
) {
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
    if hole.prompt().kind != PromptKind::Play {
        assert_eq!(hole.advice(), None, "{at}");
    }
    if let Some(action) = action {
        assert!(
            actions.iter().any(|offered| same_option(offered, action)),
            "{at}: recorded action {action:?} missing from {actions:?}"
        );
    }
}
