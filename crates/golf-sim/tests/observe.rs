use std::{collections::BTreeMap, fs, path::Path};

use golf_sim::{
    action::{Action, ReliefOption, StrokeChoice, Swing},
    hole::Hole,
    observe::{Distance, DistanceDisplay, Observation, ShotReport, observe},
    records::hole::DecisionKind,
    setup::{HoleSetup, Mode},
    stroke::results::StrokeResult,
};
use golf_sim_base::{
    ClubId, PromptKind, Real, Seed, Vec3, corpus,
    digest::object,
    variant::{self, Components, Dict, FloatWidth, Variant},
};
use golf_sim_course::course::Course;
use golf_sim_physics::values::units::{FOOT, YARD};
use golf_sim_testkit::{
    reference_records::{ReferenceEntry, decode_hole},
    tolerance,
};
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

#[test]
fn reference_observation() {
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata");
    let course = Course::open(&data.join("courses/test-course")).unwrap();
    let observations = corpus::read(&data.join("hole/observe/fixture.bin")).unwrap();
    let records = corpus::read(&data.join("records/holes/fixture.bin")).unwrap();
    let mut addresses_by_record = BTreeMap::<_, Vec<_>>::new();
    for case in cases(&observations) {
        addresses_by_record
            .entry(id(case).rsplit_once("/address/").unwrap().0)
            .or_default()
            .push(case);
    }
    let mut projections = BTreeMap::new();
    let mut boundary_count = 0;
    for record in cases(&records) {
        let address_cases = addresses_by_record.remove(id(record)).unwrap();
        let Variant::PackedByteArray(bytes) = get(dict(record, "output"), "bytes") else {
            panic!("expected record bytes");
        };
        let reference = decode_hole(bytes, &course.data().id).unwrap();
        let mode = reference.setup.settings.info_mode;
        let mut hole = Hole::start(course.clone(), reference.setup).unwrap();
        let mut addresses = address_cases.into_iter();
        let mut attempt = 0;
        let mut last_result = None;
        for entry in reference.entries.into_iter().map(Some).chain([None]) {
            if hole.prompt().kind == PromptKind::Play {
                let case = addresses
                    .next()
                    .unwrap_or_else(|| panic!("{}: missing address", id(record)));
                assert_eq!(
                    integer(dict(case, "input"), "attempt"),
                    attempt,
                    "{}",
                    id(case)
                );
                let bytes = hole.record().to_bytes().unwrap();
                let reading = observe(&hole, mode);
                assert_eq!(hole.record().to_bytes().unwrap(), bytes);
                assert_last_stroke(reading.last_stroke.as_ref(), last_result.as_ref(), id(case));
                let comparison = compare(case, &reading);
                boundary_count += comparison.boundaries;
                assert!(projections.insert(id(case), comparison).is_none());
            }
            if let Some(entry) = entry {
                let action = replay_action(entry);
                attempt += i64::from(matches!(action, Action::Stroke(_)));
                let step = hole
                    .act(action)
                    .unwrap_or_else(|error| panic!("{} attempt {attempt}: {error}", id(record)));
                if step.stroke.is_some() {
                    last_result = step.stroke;
                }
            }
        }
        assert!(addresses.next().is_none(), "{}", id(record));
    }
    assert!(addresses_by_record.is_empty());

    let mut projected = observations.as_dict().unwrap().clone();
    let Some(Variant::Array(cases)) = projected.get_mut("cases") else {
        unreachable!()
    };
    for case in &mut cases.items {
        let Variant::Dict(fields) = case else {
            unreachable!()
        };
        let comparison = &projections[id(fields)];
        *fields.get_mut("output").unwrap() = Variant::Dict(Dict::new(comparison.reference.clone()));
    }
    let body = variant::encode(&Variant::Dict(projected)).unwrap();
    let bytes = [
        b"GSCORP01".as_slice(),
        &u32::try_from(body.len()).unwrap().to_le_bytes(),
        &body,
    ]
    .concat();
    let scratch = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("observe-{}", std::process::id()))
        .join("testdata");
    fs::create_dir_all(&scratch).unwrap();
    let fixture = scratch.join("fixture.bin");
    fs::write(&fixture, bytes).unwrap();
    let tables = fs::read(data.join("tolerances.toml")).unwrap();
    assert_eq!(
        tables,
        include_bytes!("../../golf-sim-testkit/tolerances.toml")
    );
    fs::write(scratch.join("tolerances.toml"), tables).unwrap();
    let report = tolerance::check(&fixture, "hole", |case| {
        Ok::<_, std::convert::Infallible>(Variant::Dict(Dict::new(
            projections[id(case)].actual.clone(),
        )))
    });
    fs::remove_dir_all(scratch.parent().unwrap()).unwrap();
    let report = report.unwrap();
    assert_eq!(report.checked, 127);
    assert!(report.discrete.is_empty() && report.skipped.is_empty());
    println!("{report}Listed {boundary_count} rounding or cutoff differences.");
}

fn assert_last_stroke(report: Option<&ShotReport>, result: Option<&StrokeResult>, id: &str) {
    assert_eq!(report.is_some(), result.is_some(), "{id}");
    let Some((report, result)) = report.zip(result) else {
        return;
    };
    let shot = &result.playback;
    let launch = shot.launch.unwrap();
    assert_eq!(report.end, result.outcome.end, "{id}");
    assert_eq!(report.quality, result.quality, "{id}");
    assert_eq!(report.fat, result.fat, "{id}");
    assert_eq!(report.contact, launch.contact, "{id}");
    assert_eq!(
        [
            report.carry.exact_m,
            report.total.exact_m,
            report.offline.exact_m,
            report.total_offline.exact_m,
            report.apex_m,
            report.hang_time_s,
            report.land_angle_rad,
            report.ball_speed_mps,
            report.launch_angle_rad,
            report.spin_rate_rad_s,
            report.club_speed_mps,
            report.dynamic_loft_rad,
        ],
        [
            shot.carry_m(),
            shot.total_m(),
            shot.carry_side_m(),
            shot.total_side_m(),
            shot.apex_height_m(),
            shot.hang_time_s(),
            shot.land_angle_rad(),
            launch.readout.ball_speed_mps,
            launch.readout.launch_angle_rad,
            launch.readout.spin_rate_rad_s,
            launch.club_speed_mps,
            launch.dynamic_loft_rad,
        ],
        "{id}"
    );
}

#[test]
fn requested_mode_changes_the_readings_and_leaves_the_hole_replayable() {
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata");
    let course = Course::open(&data.join("courses/test-course")).unwrap();
    let mut setup = HoleSetup::standard(&course, 1, Seed(1));
    setup.settings.info_mode = Mode::Tour;
    let mut hole = Hole::start(course.clone(), setup).unwrap();
    let tee = hole.observe(Mode::Caddie);
    let address = tee.address.unwrap();
    assert!(address.preview.is_some());
    assert!(hole.observe(Mode::Tour).address.unwrap().preview.is_none());
    assert!(tee.last_stroke.is_none());

    let aim = Vec3::new(address.aim[0], address.aim[1], address.aim[2]);
    let choice = StrokeChoice::stock(ClubId::Driver, aim)
        .with_aim_direction(aim)
        .with_swing(Swing::Exact {
            power: 1.0,
            timing_fault: 0.0,
            strike_m: None,
        });
    let step = hole.act(Action::Stroke(choice)).unwrap();
    let bytes = hole.record().to_bytes().unwrap();
    let tour = hole.observe(Mode::Tour);
    let caddie = hole.observe(Mode::Caddie);
    assert_last_stroke(tour.last_stroke.as_ref(), step.stroke.as_ref(), "drive");
    assert_eq!(caddie.last_stroke, tour.last_stroke);
    assert_eq!(tour.score.strokes, 1);
    assert_eq!(tour.to_flag, caddie.to_flag);
    assert!(tour.relief_offers.is_empty());
    assert!(caddie.relief_offers.iter().any(|offer| {
        offer.option == ReliefOption::StrokeAndDistance && offer.penalty_strokes == 1
    }));
    assert_eq!(hole.record().to_bytes().unwrap(), bytes);
    assert_eq!(
        Hole::resume(course, &bytes).unwrap().observe(Mode::Tour),
        tour
    );
}

struct Comparison<'a> {
    id: &'a str,
    reference: Vec<(Variant, Variant)>,
    actual: Vec<(Variant, Variant)>,
    boundaries: usize,
}

impl Comparison<'_> {
    fn field(&mut self, name: &str, reference: Variant, actual: Variant) {
        self.reference.push((name.into(), reference));
        self.actual.push((name.into(), actual));
    }

    fn fields<const N: usize>(&mut self, from: &Dict, fields: [(&str, Variant); N]) {
        for (name, actual) in fields {
            self.field(name, get(from, name).clone(), actual);
        }
    }

    fn distance(&mut self, name: &str, reference: Real, actual: Distance) {
        self.field(name, reference.into(), actual.exact_m.into());
        let step = match actual.display {
            DistanceDisplay::Yards(_) => YARD,
            DistanceDisplay::TenthsOfFeet(_) => FOOT / 10.0,
        };
        let actual_tick = match actual.display {
            DistanceDisplay::Yards(tick) | DistanceDisplay::TenthsOfFeet(tick) => tick,
        };
        self.rounding(name, reference, actual.exact_m, step, actual_tick, 0.05);
    }

    fn rounding(
        &mut self,
        name: &str,
        reference: Real,
        actual: Real,
        step: Real,
        actual_tick: i64,
        bound: Real,
    ) {
        let expected_tick = (reference / step).round() as i64;
        assert_eq!(
            actual_tick,
            (actual / step).round() as i64,
            "{}: {name}",
            self.id
        );
        if expected_tick != actual_tick {
            let boundary = (expected_tick.min(actual_tick) as Real + 0.5) * step;
            assert_eq!(
                expected_tick.abs_diff(actual_tick),
                1,
                "{}: {name}",
                self.id
            );
            assert!(
                (reference - boundary).abs() <= bound && (actual - boundary).abs() <= bound,
                "{}: {name}",
                self.id
            );
            println!(
                "{} {name}: boundary {boundary:.9} m, Rust {actual:.9} m, reference {reference:.9} m",
                self.id
            );
            self.boundaries += 1;
        }
    }
}

fn compare<'a>(case: &'a Dict, reading: &Observation) -> Comparison<'a> {
    let mut cmp = Comparison {
        id: id(case),
        reference: Vec::new(),
        actual: Vec::new(),
        boundaries: 0,
    };
    let input = dict(case, "input");
    let output = dict(case, "output");
    let hud = dict(output, "hud");
    let address = dict(output, "address");
    let hole = dict(output, "hole");
    let ground = dict(output, "ground");
    let ball = reading.ball.unwrap();
    let wind = reading.wind.as_ref().unwrap();
    let stance = &reading.address.as_ref().unwrap().stance;
    let read = reading.address.as_ref().unwrap();
    cmp.fields(
        output,
        [
            ("ball_position", vector(ball.spot)),
            ("cup", vector(reading.cup)),
        ],
    );
    let reference_spot = coordinates(get(output, "ball_position"));
    let reference_aim = coordinates(get(output, "aim"));
    let aim_range = number(hud, "target_distance_m");
    let reference_aim_point =
        std::array::from_fn(|axis| reference_spot[axis] + reference_aim[axis] * aim_range);
    cmp.field(
        "aim_point",
        vector(reference_aim_point),
        vector(read.aim_point),
    );
    cmp.field(
        "mode",
        get(input, "info_mode").clone(),
        (reading.mode as i64).into(),
    );
    cmp.distance(
        "target_distance_m",
        number(hud, "target_distance_m"),
        reading.to_flag.unwrap(),
    );
    assert_eq!(
        matches!(
            reading.to_flag.unwrap().display,
            DistanceDisplay::TenthsOfFeet(_)
        ),
        reading.to_flag.unwrap().exact_m < 12.0,
        "{}",
        cmp.id,
    );
    if number(hud, "range_distance_m").is_finite() {
        cmp.distance(
            "range_distance_m",
            number(hud, "range_distance_m"),
            reading.to_cup.unwrap(),
        );
        cmp.fields(hud, [("range_rise_m", optional(reading.flag_rise_m))]);
    }
    cmp.fields(
        hud,
        [
            ("bag_index", (read.club as i64).into()),
            ("face_half_width_m", read.face_half_width_m.into()),
            ("face_height_m", read.face_height_m.into()),
            ("face_is_wood", Variant::Bool(read.face_is_wood)),
            ("putting", Variant::Bool(read.putting)),
            ("flag_wind_mps", vector(wind.flag_mps)),
            ("felt_wind_mph", wind.felt_mph.into()),
            (
                "air_temperature_c",
                optional(wind.displayed_temperature_c.map(|t| t as Real)),
            ),
            (
                "stock_carry_m",
                optional(read.stock_carry.map(|d| d.exact_m)),
            ),
        ],
    );
    cmp.fields(
        address,
        [
            ("flagstick_in", Variant::Bool(read.flagstick_in)),
            ("club_length_m", read.club_length_m.into()),
            ("club_lie_rad", read.club_lie_rad.into()),
            ("slope_along_rad", stance.slope_along_rad.into()),
            ("slope_across_rad", stance.slope_across_rad.into()),
        ],
    );
    cmp.fields(
        dict(input, "ball_lie"),
        [
            ("surface", (ball.surface as i64).into()),
            ("sit", (ball.sit as i64).into()),
            ("divot", (ball.divot as i64).into()),
            ("ball_height_m", ball.ball_height_m.into()),
            ("grass_height_m", ball.grass_height_m.into()),
            ("grass_density", ball.grass_density.into()),
            ("sand_firmness", ball.sand_firmness.into()),
        ],
    );
    cmp.fields(
        dict(input, "address_lie"),
        [
            ("ball_slope_along_rad", stance.ball_slope_along_rad.into()),
            (
                "lead_foot_surface",
                (stance.lead_foot_surface as i64).into(),
            ),
            (
                "trail_foot_surface",
                (stance.trail_foot_surface as i64).into(),
            ),
            (
                "club_entry_surface",
                (stance.club_entry_surface as i64).into(),
            ),
        ],
    );
    cmp.fields(
        dict(input, "wind"),
        [
            ("felt_mps", vector(wind.felt_vector_mps)),
            ("ground_mps", vector(wind.ground_mps)),
            ("under_trees", Variant::Bool(wind.under_trees)),
        ],
    );
    cmp.fields(ground, [("wind_mps", vectors(wind.at_heights_mps.clone()))]);
    let score = &reading.score;
    cmp.fields(
        hole,
        [
            ("number", score.number.into()),
            ("par", score.par.into()),
            ("length_yd", score.length_yd.into()),
            ("strokes", (score.strokes as i64).into()),
            ("stroke", (score.next_stroke.unwrap() as i64).into()),
            ("penalties", (score.penalties as i64).into()),
            ("prompt", (reading.prompt as i64).into()),
            ("provisional", Variant::Bool(score.provisional)),
            ("on_green", Variant::Bool(reading.green.is_some())),
            ("relief_available", Variant::Bool(reading.relief_available)),
            (
                "green_speed_m",
                optional(
                    reading
                        .green
                        .as_ref()
                        .and_then(|g| g.overlay.as_ref())
                        .map(|g| g.speed_m),
                ),
            ),
            (
                "landing",
                vector(read.landing.unwrap_or([Real::INFINITY; 3])),
            ),
        ],
    );
    let Variant::Array(card) = get(hole, "card") else {
        unreachable!()
    };
    let actual_card = score
        .card
        .iter()
        .map(|shot| {
            object([
                ("number", (shot.number as i64).into()),
                ("from", vector(shot.from)),
                ("rest", vector(shot.rest)),
                ("distance_m", shot.distance.exact_m.into()),
                ("rest_surface", (shot.rest_surface as i64).into()),
                ("penalty_strokes", (shot.penalties as i64).into()),
            ])
        })
        .collect();
    cmp.field(
        "card",
        Variant::Array(card.clone()),
        Variant::Array(variant::Array::new(actual_card)),
    );
    compare_preview(&mut cmp, input, hud, read.preview.as_ref());
    cmp
}

fn compare_preview(
    cmp: &mut Comparison<'_>,
    input: &Dict,
    hud: &Dict,
    preview: Option<&golf_sim::observe::PreviewReading>,
) {
    let source = get(input, "preview").as_dict();
    if let Some((source, preview)) = source.zip(preview) {
        cmp.fields(
            source,
            [
                ("plays_like_m", optional(preview.plays_like_m)),
                ("carry_m", optional(preview.carry.map(|d| d.exact_m))),
                ("carry_to_target_m", optional(preview.carry_to_target_m)),
                ("aloft", (preview.aloft as i64).into()),
                ("wind_aloft_mps", vector(preview.wind_aloft_mps)),
            ],
        );
        if let Some(yd) = preview.displayed_plays_like_yd {
            cmp.rounding(
                "displayed_plays_like_m",
                number(source, "plays_like_m"),
                preview.plays_like_m.unwrap(),
                5.0 * YARD,
                yd / 5,
                0.1,
            );
        }
        let displayed = preview.displayed_plays_like_yd.map(|yd| yd as Real * YARD);
        let recorded = number(hud, "plays_like_m");
        assert_eq!(
            recorded.is_finite(),
            displayed.is_some(),
            "{} plays-like visibility",
            cmp.id
        );
        cmp.fields(
            hud,
            [
                (
                    "nominal_carry_m",
                    optional(preview.nominal_carry.map(|d| d.exact_m)),
                ),
                (
                    "nominal_roll_m",
                    optional(preview.nominal_roll.map(|d| d.exact_m)),
                ),
            ],
        );
        let Variant::PackedVector3Array(reference) = get(hud, "putt_line") else {
            unreachable!()
        };
        let actual = &preview.putt_line;
        let shared = actual.len().min(reference.values.len());
        if actual.len() != reference.values.len() {
            assert_eq!(
                actual.len().abs_diff(reference.values.len()),
                1,
                "{} putt line",
                cmp.id
            );
            let range = |p: [Real; 3], start: [Real; 3]| {
                (Vec3::new(p[0], p[1], p[2]) - Vec3::new(start[0], start[1], start[2])).length()
            };
            let rust = range(actual[shared - 1], actual[0]);
            let oracle = range(reference.values[shared - 1], reference.values[0]);
            let rust_last = range(*actual.last().unwrap(), actual[0]);
            let oracle_last = range(*reference.values.last().unwrap(), reference.values[0]);
            assert!(
                (rust - 0.9).abs() <= 0.05
                    && (oracle - 0.9).abs() <= 0.05
                    && (rust < 0.9) == (actual.len() > reference.values.len())
                    && (oracle < 0.9) == (reference.values.len() > actual.len()),
                "{} putt line",
                cmp.id
            );
            println!(
                "{} putt line: cutoff 0.9 m, shared Rust {rust:.9} m and reference {oracle:.9} m, last Rust {rust_last:.9} m and reference {oracle_last:.9} m",
                cmp.id
            );
            cmp.boundaries += 1;
        }
        cmp.field(
            "putt_line",
            vectors(reference.values[..shared].to_vec()),
            vectors(actual[..shared].to_vec()),
        );
    } else {
        assert!(
            matches!(get(hud, "putt_line"), Variant::PackedVector3Array(points) if points.values.is_empty())
        );
    }
}

fn vector(values: [Real; 3]) -> Variant {
    Variant::Vector3(Components {
        values,
        width: FloatWidth::Double,
    })
}

fn coordinates(value: &Variant) -> [Real; 3] {
    let Variant::Vector3(value) = value else {
        panic!("expected a vector");
    };
    value.values
}

fn vectors(values: Vec<[Real; 3]>) -> Variant {
    Variant::PackedVector3Array(Components {
        values,
        width: FloatWidth::Double,
    })
}

fn optional(value: Option<Real>) -> Variant {
    value.unwrap_or(Real::NAN).into()
}

fn number(dict: &Dict, key: &str) -> Real {
    let Variant::Float(n) = get(dict, key) else {
        panic!("{key}: expected float")
    };
    n.value()
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
