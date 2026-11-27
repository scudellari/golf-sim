use std::{collections::BTreeSet, path::Path};

use golf_sim_base::{
    PromptKind, Real, Surface, Vec2, Vec3,
    digest::object,
    trace::NoProbe,
    variant::{self, Array, Components, Dict, FloatWidth, Variant},
};
use golf_sim_course::{
    course::{Course, PenaltyColor},
    legal::Area,
};
use golf_sim_rules::{
    relief::{self, AbnormalCondition, AbnormalKind},
    rulings::{
        Consequence, HoleRules, PenaltyAreaValue, ReliefArea, ReliefOption, RulesRefusal,
        RulesStep, Ruling, Status,
    },
    transitions,
};
use golf_sim_stroke::results::{DropResult, End, StrokeOutcome};
use golf_sim_testkit::tolerance;

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
    let report = tolerance::check(&path, "rules", |case| {
        let call = field(case, "call").as_str().unwrap();
        calls.insert(call.to_owned());
        relief_cases += usize::from(RELIEF_CALLS.contains(&call));
        checked_replay(case, replay(call, dict(field(case, "input"))))
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
    assert_eq!(report.checked, 585);
    assert_eq!(relief_cases, 83);
    assert!(report.discrete.is_empty());
    assert!(report.skipped.is_empty());
    print!("{report}");
    println!(
        "All 585 transition cases matched the rules tolerance, including 83 relief transitions."
    );
}

pub(crate) fn checked_replay(case: &Dict, actual: Variant) -> Result<Variant, &'static str> {
    if integers_match(field(case, "output"), &actual) {
        Ok(actual)
    } else {
        Err("integer output differs")
    }
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

pub(crate) fn replay(call: &str, input: &Dict) -> Variant {
    let get = |key| field(input, key);
    let rules = || read_rules(get("rules"));
    match call {
        "rules.start_hole" => step_value(&transitions::start_hole(point3(get("tee_spot")))),
        "rules.after_stroke" => {
            let course = legal_course(dict(get("legal")));
            step_value(&transitions::after_stroke(
                &rules(),
                &read_outcome(get("outcome")),
                &course.legal(),
                optional_point(get("hole")),
                usize::try_from(integer(get("attempt"))).ok(),
                boolean(get("doubtful")),
            ))
        }
        "rules.status" => {
            let course = legal_course(dict(get("legal")));
            enum_value(
                STATUSES,
                transitions::status(&read_outcome(get("outcome")), &course.legal()),
            )
        }
        "rules.consequence" => {
            let status = usize::try_from(integer(get("found")))
                .ok()
                .and_then(|id| STATUSES.get(id));
            // An invalid wire status has no corresponding Rust enum variant.
            status.map_or_else(
                || (-1).into(),
                |&status| enum_value(CONSEQUENCES, transitions::consequence(status)),
            )
        }
        "rules.go_forward" => step_value(&transitions::go_forward(
            &rules(),
            nullable(get("area"), read_relief),
            choice(AREAS, get("from_area")),
        )),
        "rules.provisional_available" => {
            Variant::Bool(transitions::provisional_available(&rules()))
        }
        "rules.declare_provisional" => step_value(&transitions::declare_provisional(
            &rules(),
            read_relief(get("area")),
            choice(AREAS, get("from_area")),
        )),
        "rules.penalty_options" => array(
            transitions::penalty_options(&rules())
                .into_iter()
                .map(|option| enum_value(OPTIONS, option)),
        ),
        "rules.relief_available" => Variant::Bool(relief::relief_available(
            &rules(),
            &legal_course(dict(get("legal"))).legal(),
            &read_relief(get("area")),
            &read_condition(get("condition")),
        )),
        "rules.ask_relief" => step_value(&relief::ask_relief(
            &rules(),
            read_relief(get("area")),
            &legal_course(dict(get("legal"))).legal(),
            &mut NoProbe,
        )),
        "rules.unplayable_options" => array(
            relief::unplayable_options(
                &rules(),
                boolean(get("in_bunker")),
                boolean(get("in_penalty")),
            )
            .into_iter()
            .map(|option| enum_value(OPTIONS, option)),
        ),
        "rules.declare_unplayable" => step_value(&relief::declare_unplayable(
            &rules(),
            read_relief(get("area")),
            &legal_course(dict(get("legal"))).legal(),
            choice(AREAS, get("from_area")),
            &mut NoProbe,
        )),
        "rules.embedded_available" => Variant::Bool(relief::embedded_available(
            &rules(),
            &read_relief(get("area")),
            &legal_course(dict(get("legal"))).legal(),
        )),
        "rules.embedded_relief" => step_value(&relief::embedded_relief(
            &rules(),
            read_relief(get("area")),
            &legal_course(dict(get("legal"))).legal(),
            &mut NoProbe,
        )),
        "rules.after_drop" => {
            let drop = dict(get("drop"));
            step_value(&relief::after_drop(
                &rules(),
                &DropResult {
                    first: optional_point(field(drop, "first")),
                    rest: optional_point(field(drop, "rest")),
                },
                &legal_course(dict(get("legal"))).legal(),
                &mut NoProbe,
            ))
        }
        "rules.place" => step_value(&relief::place(&rules(), point3(get("spot")), &mut NoProbe)),
        "rules.choose_penalty" => step_value(&relief::choose_penalty(
            &rules(),
            read_relief(get("area")),
            choice(AREAS, get("from_area")),
            &mut NoProbe,
        )),
        _ => panic!("unexpected call {call}"),
    }
}

const STATUSES: &[Status] = &[
    Status::Holed,
    Status::OutOfBounds,
    Status::Lost,
    Status::InWater,
    Status::InPlay,
    Status::PenaltyArea,
    Status::Doubtful,
];
const CONSEQUENCES: &[Consequence] = &[
    Consequence::Holed,
    Consequence::AsItLies,
    Consequence::MaybeLost,
    Consequence::LastSpot,
    Consequence::Drop,
    Consequence::Place,
    Consequence::Relief,
    Consequence::Choose,
    Consequence::ProvisionalPlay,
];
const PROMPTS: &[PromptKind] = &[
    PromptKind::Play,
    PromptKind::MaybeLost,
    PromptKind::Done,
    PromptKind::Choose,
    PromptKind::Drop,
    PromptKind::Place,
];
const OPTIONS: &[ReliefOption] = &[
    ReliefOption::Cancel,
    ReliefOption::StrokeAndDistance,
    ReliefOption::BackOnLine,
    ReliefOption::Lateral,
    ReliefOption::OutsideBunker,
    ReliefOption::TemporaryWater,
    ReliefOption::GroundUnderRepair,
    ReliefOption::ImmovableObstruction,
    ReliefOption::Embedded,
    ReliefOption::BunkerAbnormalOutside,
    ReliefOption::Provisional,
];
const AREAS: &[Area] = &[
    Area::General,
    Area::Teeing,
    Area::Green,
    Area::Bunker,
    Area::Penalty,
];
const ENDS: &[End] = &[End::AtRest, End::Holed, End::InWater, End::TimedOut];
pub(crate) const NO_POINT: Vec3 = Vec3::new(Real::INFINITY, Real::INFINITY, Real::INFINITY);

fn read_outcome(value: &Variant) -> StrokeOutcome {
    let get = |key| field(dict(value), key);
    StrokeOutcome {
        end: choice(ENDS, get("end")),
        start: point3(get("start")),
        rest: point3(get("rest")),
        rest_surface: choice(Surface::ALL, get("rest_surface")),
        ball_radius_m: real(get("ball_radius_m")),
        carry_m: real(get("carry_m")),
        total_m: real(get("total_m")),
        embedded: boolean(get("embedded")),
        penalty_entry: optional_point(get("penalty_entry")),
        marks: Vec::new(),
    }
}

pub(crate) fn read_rules(value: &Variant) -> HoleRules {
    let rules = dict(value);
    HoleRules {
        tee: point3(field(rules, "tee")),
        rulings: items(field(rules, "rulings"))
            .iter()
            .map(read_ruling)
            .collect(),
    }
}

fn read_ruling(value: &Variant) -> Ruling {
    let get = |key| field(dict(value), key);
    Ruling {
        status: choice(STATUSES, get("status")),
        consequence: choice(CONSEQUENCES, get("next")),
        penalty_strokes: usize::try_from(integer(get("penalty_strokes"))).unwrap(),
        reason: None,
        relief: nullable(get("relief"), read_relief),
        drops: usize::try_from(integer(get("drops"))).unwrap(),
        place_at: optional_point(get("place_at")),
        penalty_area: nullable(get("penalty_area"), read_penalty),
        penalty_entry: optional_point(get("penalty_entry")),
        playable_in_penalty_area: boolean(get("playable_in_penalty_area")),
        embedded: boolean(get("embedded")),
        attempt: usize::try_from(integer(get("attempt"))).ok(),
        provisional: nullable(get("provisional"), |rules| Box::new(read_rules(rules))),
        provisional_from_last: boolean(get("provisional_from_last")),
        from: point3(get("from")),
        rest: point3(get("rest")),
        spot: optional_point(get("spot")),
    }
}

pub(crate) fn read_relief(value: &Variant) -> ReliefArea {
    let get = |key| field(dict(value), key);
    ReliefArea {
        option: choice(OPTIONS, get("option")),
        reference: point3(get("reference")),
        hole: point3(get("hole")),
        club_length_m: real(get("club_length_m")),
        bunker: outline(get("bunker")),
        penalty_area: nullable(get("penalty_area"), read_penalty),
        tee: outline(get("tee")),
        green: outline(get("green")),
        required_area: id_of(get("required_area")).map(|id| AREAS[id]),
        green_placement: boolean(get("green_placement")),
        condition: nullable(get("condition"), read_condition),
        footprint: outline(get("footprint")),
        ball_radius_m: real(get("ball_radius_m")),
    }
}

pub(crate) fn read_kind(value: &Variant) -> AbnormalKind {
    match value.as_str().unwrap() {
        "Temporary water" => AbnormalKind::TemporaryWater,
        "Ground under repair" => AbnormalKind::GroundUnderRepair,
        "Immovable obstruction" => AbnormalKind::ImmovableObstruction,
        label => panic!("unknown condition {label}"),
    }
}

pub(crate) fn read_condition(value: &Variant) -> AbnormalCondition {
    let condition = dict(value);
    AbnormalCondition::new(
        read_kind(field(condition, "name")),
        outline(field(condition, "outline")),
    )
}

pub(crate) fn condition_value(condition: &AbnormalCondition) -> Variant {
    object([
        ("name", condition.kind.name().into()),
        ("outline", outline_value(&condition.outline)),
    ])
}

fn read_penalty(value: &Variant) -> PenaltyAreaValue {
    let area = dict(value);
    PenaltyAreaValue {
        outline: outline(field(area, "outline")),
        color: if boolean(field(area, "red")) {
            PenaltyColor::Red
        } else {
            PenaltyColor::Yellow
        },
    }
}

fn step_value(step: &RulesStep) -> Variant {
    let prompt = step.prompt();
    let refusal = match step.refusal {
        None => String::new(),
        Some(RulesRefusal::StrokeWhileFinishedOrPending) => {
            "a stroke was played while the hole was finished or a ruling was pending".into()
        }
        Some(RulesRefusal::WrongStrokeSpot { start, ball }) => {
            let point = |p: Vec3| format!("({:?}, {:?}, {:?})", p.x, p.y, p.z);
            format!(
                "the stroke started at {}, but the ball lies at {}",
                point(start),
                point(ball)
            )
        }
        Some(RulesRefusal::NoRulingPending) => {
            "the player went forward with no ruling pending".into()
        }
        Some(RulesRefusal::ProvisionalNotPlayed) => "the provisional has not been played".into(),
        Some(RulesRefusal::ProvisionalUnavailable) => "a provisional ball is unavailable".into(),
        Some(RulesRefusal::NoConditionInterference) => {
            "the condition does not interfere with play".into()
        }
        Some(RulesRefusal::NotInBunker) => "the ball is not in a bunker".into(),
        Some(RulesRefusal::NoCompleteRelief) => "there is no point of complete relief".into(),
        Some(RulesRefusal::UnplayableOptionUnavailable) => {
            "that unplayable-ball option is unavailable".into()
        }
        Some(RulesRefusal::EmbeddedBallUnavailable) => {
            "there is no embedded ball in the general area".into()
        }
        Some(RulesRefusal::NoDropPending) => "no drop is pending".into(),
        Some(RulesRefusal::DropFirstOutsideReliefArea) => {
            "the drop first touched outside the relief area".into()
        }
        Some(RulesRefusal::WrongPlacementSpot) => {
            "the ball must be placed at the required spot".into()
        }
        Some(RulesRefusal::PenaltyOptionUnavailable) => {
            "that penalty-area option is unavailable".into()
        }
        Some(RulesRefusal::HoleFinished) => panic!("pick-up has no fixture call"),
    };
    object([
        ("rules", rules_value(&step.rules)),
        (
            "prompt",
            object([
                ("kind", enum_value(PROMPTS, prompt.kind)),
                ("ruling", prompt.ruling.map_or(Variant::Nil, ruling_value)),
            ]),
        ),
        ("refusal", Variant::String(refusal)),
    ])
}

fn rules_value(rules: &HoleRules) -> Variant {
    object([
        ("tee", point_value(rules.tee)),
        ("rulings", array(rules.rulings.iter().map(ruling_value))),
    ])
}

fn ruling_value(ruling: &Ruling) -> Variant {
    object([
        ("status", enum_value(STATUSES, ruling.status)),
        ("next", enum_value(CONSEQUENCES, ruling.consequence)),
        ("penalty_strokes", (ruling.penalty_strokes as i64).into()),
        (
            "relief",
            ruling.relief.as_ref().map_or(Variant::Nil, relief_value),
        ),
        ("drops", (ruling.drops as i64).into()),
        ("place_at", point_value(ruling.place_at.unwrap_or(NO_POINT))),
        (
            "penalty_area",
            ruling
                .penalty_area
                .as_ref()
                .map_or(Variant::Nil, penalty_value),
        ),
        (
            "penalty_entry",
            point_value(ruling.penalty_entry.unwrap_or(NO_POINT)),
        ),
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
        ("spot", point_value(ruling.spot.unwrap_or(NO_POINT))),
    ])
}

fn relief_value(area: &ReliefArea) -> Variant {
    object([
        ("option", enum_value(OPTIONS, area.option)),
        ("reference", point_value(area.reference)),
        ("hole", point_value(area.hole)),
        ("club_length_m", area.club_length_m.into()),
        ("bunker", outline_value(&area.bunker)),
        (
            "penalty_area",
            area.penalty_area
                .as_ref()
                .map_or(Variant::Nil, penalty_value),
        ),
        ("tee", outline_value(&area.tee)),
        ("green", outline_value(&area.green)),
        (
            "required_area",
            area.required_area
                .map_or_else(|| (-1).into(), |area| enum_value(AREAS, area)),
        ),
        ("green_placement", Variant::Bool(area.green_placement)),
        (
            "condition",
            area.condition
                .as_ref()
                .map_or(Variant::Nil, condition_value),
        ),
        ("footprint", outline_value(&area.footprint)),
        ("ball_radius_m", area.ball_radius_m.into()),
    ])
}

fn penalty_value(area: &PenaltyAreaValue) -> Variant {
    object([
        ("outline", outline_value(&area.outline)),
        ("red", Variant::Bool(area.color == PenaltyColor::Red)),
    ])
}

pub(crate) fn legal_course(legal: &Dict) -> std::sync::Arc<Course> {
    let get = |key| items(field(legal, key));
    let bounds = get("in_bounds").iter().map(surface_tile).collect();
    let mut regions = Vec::new();
    for (key, kind) in [
        ("out_of_bounds", "out_of_bounds"),
        ("lost_ball", "lost_ball"),
    ] {
        regions.extend(get(key).iter().map(|polygon| region(kind, polygon, "red")));
    }
    regions.extend(get("penalty_areas").iter().map(|area| {
        let area = dict(area);
        let color = if boolean(field(area, "red")) {
            "red"
        } else {
            "yellow"
        };
        region("penalty_area", field(area, "outline"), color)
    }));
    let bunkers = get("bunkers")
        .iter()
        .map(|polygon| {
            object([
                ("id", "".into()),
                ("hole", 0.into()),
                ("outline", polygon.clone()),
                ("floor_m", 0.0.into()),
            ])
        })
        .collect();
    synthetic_course(bounds, regions, bunkers)
}

fn region(kind: &str, polygon: &Variant, color: &str) -> Variant {
    object([
        ("id", "".into()),
        ("legal", kind.into()),
        ("penalty_color", color.into()),
        ("hole", 0.into()),
        ("polygon", polygon.clone()),
    ])
}

fn surface_tile(polygon: &Variant) -> Variant {
    let [origin, right, end, left]: [Vec2; 4] = outline(polygon).try_into().unwrap();
    assert_eq!(right, Vec2::new(end.x, origin.y));
    assert_eq!(left, Vec2::new(origin.x, end.y));

    // LegalMap takes baked tile spans from CourseData. Recreate those spans from the outlines.
    let span = end - origin;
    let mut cell = span.x;
    let mut remainder = span.y;
    while remainder != 0.0 {
        (cell, remainder) = (remainder, cell % remainder);
    }
    let size = [(span.x / cell) as i32 + 1, (span.y / cell) as i32 + 1];
    assert_eq!(
        Vec2::new(
            origin.x + Real::from(size[0] - 1) * cell,
            origin.y + Real::from(size[1] - 1) * cell,
        ),
        end
    );
    object([
        ("origin", Variant::Vector2(double([origin.x, origin.y]))),
        ("cell_m", cell.into()),
        ("size", Variant::Vector2i(size)),
        (
            "materials",
            Variant::PackedByteArray(vec![0; (size[0] * size[1]) as usize]),
        ),
    ])
}

fn synthetic_course(
    mut bounds: Vec<Variant>,
    regions: Vec<Variant>,
    bunkers: Vec<Variant>,
) -> std::sync::Arc<Course> {
    let empty = || array([]);
    let terrain = object([
        ("origin", Variant::Vector2(double([0.0, 0.0]))),
        ("cell_m", 1.0.into()),
        ("size", Variant::Vector2i([4, 4])),
        ("heights", Variant::PackedFloat32Array(vec![0.0; 16])),
    ]);
    if bounds.is_empty() {
        bounds.push(surface_tile(&outline_value(&[
            Vec2::ZERO,
            Vec2::new(100.0, 0.0),
            Vec2::new(100.0, 100.0),
            Vec2::new(0.0, 100.0),
        ])));
    }
    let material = object([
        ("id", "fairway".into()),
        ("kind", "FAIRWAY".into()),
        ("grass_height_m", 0.0.into()),
        ("grass", "".into()),
        ("soil", "".into()),
        ("drainage", "".into()),
    ]);
    let payload = object([
        ("format", "links-lab-course".into()),
        ("format_version", 1.into()),
        ("bake_version", "0.6.0".into()),
        (
            "course",
            object([
                ("id", "fixture".into()),
                ("name", "Fixture".into()),
                ("location", "".into()),
            ]),
        ),
        ("source_hash", "".into()),
        ("frame", object([("axes", "".into())])),
        ("climate", object([])),
        ("maintenance", object([])),
        ("irrigation", object([])),
        ("materials", array([material])),
        ("terrain_tiles", array([terrain])),
        ("fine_tiles", empty()),
        ("surface_tiles", array(bounds)),
        ("regions", array(regions)),
        ("bunkers", array(bunkers)),
        ("bunker_faces", empty()),
        ("walls", empty()),
        ("rocks", empty()),
        (
            "trees",
            object([
                ("species", empty()),
                ("id", Variant::PackedStringArray(Vec::new())),
                ("species_index", Variant::PackedByteArray(Vec::new())),
                ("base", Variant::PackedVector3Array(double(Vec::new()))),
                ("height_m", Variant::PackedFloat32Array(Vec::new())),
                ("crown_radius_m", Variant::PackedFloat32Array(Vec::new())),
                ("crown_base_m", Variant::PackedFloat32Array(Vec::new())),
                ("trunk_radius_m", Variant::PackedFloat32Array(Vec::new())),
                ("density", Variant::PackedFloat32Array(Vec::new())),
                ("cone", Variant::PackedByteArray(Vec::new())),
            ]),
        ),
        ("holes", empty()),
        (
            "features",
            object([(
                "clubhouse",
                object([
                    ("position", point_value(Vec3::ZERO)),
                    ("size_m", Variant::Vector2(double([0.0, 0.0]))),
                    ("yaw_rad", 0.0.into()),
                ]),
            )]),
        ),
        ("grade_bound", 0.0.into()),
        ("turf_tiles", empty()),
    ]);
    Course::parse(&variant::encode(&payload).unwrap()).unwrap()
}

pub(crate) fn integers_match(expected: &Variant, actual: &Variant) -> bool {
    match (expected, actual) {
        (Variant::Int(a), Variant::Int(b)) => a.value() == b.value(),
        (Variant::Dict(a), Variant::Dict(b)) => a.entries().iter().all(|(key, value)| {
            b.get(key.as_str().unwrap())
                .is_some_and(|actual| integers_match(value, actual))
        }),
        (Variant::Array(a), Variant::Array(b)) => a
            .items
            .iter()
            .zip(&b.items)
            .all(|(a, b)| integers_match(a, b)),
        _ => true,
    }
}

fn array(values: impl IntoIterator<Item = Variant>) -> Variant {
    Variant::Array(Array::new(values.into_iter().collect()))
}

pub(crate) fn double<T>(values: T) -> Components<T> {
    Components {
        values,
        width: FloatWidth::Double,
    }
}

pub(crate) fn point_value(p: Vec3) -> Variant {
    Variant::Vector3(double([p.x, p.y, p.z]))
}

pub(crate) fn outline_value(points: &[Vec2]) -> Variant {
    Variant::PackedVector2Array(double(points.iter().map(|p| [p.x, p.y]).collect()))
}

pub(crate) fn dict(value: &Variant) -> &Dict {
    value.as_dict().expect("expected a dictionary")
}

pub(crate) fn field<'a>(dict: &'a Dict, key: &str) -> &'a Variant {
    dict.get(key)
        .unwrap_or_else(|| panic!("missing field {key}"))
}

pub(crate) fn items(value: &Variant) -> &[Variant] {
    let Variant::Array(array) = value else {
        panic!("expected an array")
    };
    &array.items
}

pub(crate) fn integer(value: &Variant) -> i64 {
    let Variant::Int(n) = value else {
        panic!("expected an integer")
    };
    n.value()
}

pub(crate) fn real(value: &Variant) -> Real {
    let Variant::Float(n) = value else {
        panic!("expected a float")
    };
    n.value()
}

fn boolean(value: &Variant) -> bool {
    let Variant::Bool(flag) = value else {
        panic!("expected a boolean")
    };
    *flag
}

pub(crate) fn point3(value: &Variant) -> Vec3 {
    let Variant::Vector3(v) = value else {
        panic!("expected a Vector3")
    };
    let [x, y, z] = v.values;
    Vec3::new(x, y, z)
}

pub(crate) fn outline(value: &Variant) -> Vec<Vec2> {
    let Variant::PackedVector2Array(points) = value else {
        panic!("expected an outline")
    };
    points
        .values
        .iter()
        .map(|&[x, y]| Vec2::new(x, y))
        .collect()
}

fn optional_point(value: &Variant) -> Option<Vec3> {
    let point = point3(value);
    (point != NO_POINT).then_some(point)
}

fn nullable<T>(value: &Variant, read: impl FnOnce(&Variant) -> T) -> Option<T> {
    (!matches!(value, Variant::Nil)).then(|| read(value))
}

fn id_of(value: &Variant) -> Option<usize> {
    usize::try_from(integer(value)).ok()
}

fn choice<T: Copy>(choices: &[T], value: &Variant) -> T {
    choices[id_of(value).unwrap()]
}

fn enum_value<T: PartialEq>(choices: &[T], value: T) -> Variant {
    (choices.iter().position(|choice| *choice == value).unwrap() as i64).into()
}
