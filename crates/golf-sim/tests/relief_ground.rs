use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use golf_sim::{
    hole::relief_ground::{self, ReliefOutlines},
    setup::{HoleSetup, PlaySettings},
};
use golf_sim_base::{
    Real, Vec2, Vec3, corpus,
    digest::object,
    geometry::is_point_in_polygon,
    variant::{self, Array, Components, Dict, FloatWidth, Variant},
};
use golf_sim_conditions::{grounds::CourseGround, world::World};
use golf_sim_course::{
    course::{Course, PenaltyColor},
    legal::{Area, LegalMap},
};
use golf_sim_rules::{
    relief::{self, AbnormalCondition, AbnormalKind, ReliefArea, ReliefOption, flat},
    rulings::PenaltyAreaValue,
};
use golf_sim_testkit::tolerance;

#[test]
fn relief_ground_matches_reference() {
    let testdata = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata");
    let fixture = corpus::read(&testdata.join("rules/relief/fixture.bin")).unwrap();
    let cases = items(field(dict(&fixture), "cases"));
    let course = Course::open(&testdata.join("courses/test-course")).unwrap();
    let legal = course.legal();
    let mut paths = CartPaths::default();
    let mut comparisons = Comparisons::default();
    let mut unavailable = Vec::new();

    let mut world_cases: Vec<_> = cases
        .iter()
        .map(dict)
        .filter(|case| label(case, "call").starts_with("relief_ground."))
        .collect();
    let world_count = world_cases.len();
    assert_eq!(world_count, 23);
    // Rebuild the cart paths before replaying drops that carry those outlines.
    world_cases.sort_by_key(|case| label(case, "call") == "relief_ground.drop_spots");
    for case in world_cases {
        let input = dict(field(case, "input"));
        match FixtureWorld::read(input, &course) {
            Ok(start) => {
                let (expected, actual) = replay_world(case, &start, &course, &mut paths);
                comparisons.add(case, expected, actual);
            }
            Err(reason) => unavailable.push(format!("{}: {reason}", label(case, "id"))),
        }
    }
    assert!(
        unavailable.is_empty(),
        "could not rebuild recorded worlds:\n{}",
        unavailable.join("\n")
    );

    let mut cart_path_replays = 0;
    for case in cases.iter().map(dict) {
        if let Some((expected, actual)) = replay_cart_path(case, &legal, &paths) {
            comparisons.add(case, expected, actual);
            cart_path_replays += 1;
        }
    }
    assert_eq!(cart_path_replays, 113);
    comparisons.check(&testdata, dict(&fixture));
    println!(
        "Matched {world_count} relief-ground cases and {cart_path_replays} dependent cart-path checks."
    );
    println!(
        "Listed {} cart-path outline differences.",
        paths.differences
    );
}

fn replay_world(
    case: &Dict,
    start: &FixtureWorld<'_>,
    course: &Course,
    paths: &mut CartPaths,
) -> (Variant, Variant) {
    let input = dict(field(case, "input"));
    let recorded = field(case, "output");
    let id = label(case, "id");
    match label(case, "call") {
        "relief_ground.tee_outline" => (
            position(recorded.clone()),
            position(outline_value(relief_ground::tee_outline(
                &start.setup,
                course,
            ))),
        ),
        "relief_ground._cart_path_outlines" => {
            let query = point2(field(input, "point"));
            let mut expected: Vec<_> = items(recorded).iter().map(outline).collect();
            let mut actual = relief_ground::cart_path_outlines(course, query);
            assert_eq!(
                expected.len(),
                actual.len(),
                "{id}: cart-path outline count"
            );
            for (index, (expected, actual)) in expected.iter_mut().zip(&mut actual).enumerate() {
                paths.compare(id, index, query, expected, actual);
            }
            (
                outline_positions(expected.iter().map(Vec::as_slice)),
                outline_positions(actual.iter().map(Vec::as_slice)),
            )
        }
        "relief_ground.conditions" => {
            let query = point3(field(input, "point"));
            let mut expected: Vec<_> = items(recorded).iter().map(read_condition).collect();
            let mut actual = relief_ground::conditions(course, query, start.marked());
            assert_eq!(expected.len(), actual.len(), "{id}: condition count");
            for (index, (expected, actual)) in expected.iter_mut().zip(&mut actual).enumerate() {
                assert_eq!(expected.kind, actual.kind, "{id}: condition {index} kind");
                if expected.kind == AbnormalKind::ImmovableObstruction {
                    paths.compare(
                        id,
                        index,
                        flat(query),
                        &mut expected.outline,
                        &mut actual.outline,
                    );
                }
            }
            let positions = |conditions: &[AbnormalCondition]| {
                outline_positions(
                    conditions
                        .iter()
                        .map(|condition| condition.outline.as_slice()),
                )
            };
            (positions(&expected), positions(&actual))
        }
        "relief_ground.drop_spots" => {
            let mut area = read_relief(field(input, "area"));
            if area.option == ReliefOption::ImmovableObstruction {
                assert!(
                    paths.substitute_area(&mut area),
                    "{id}: missing computed cart path"
                );
            }
            let spots = relief_ground::drop_spots(&area, &start.world, course);
            (position(recorded.clone()), position(spots_value(spots)))
        }
        call => panic!("{id}: unexpected relief ground call {call}"),
    }
}

fn replay_cart_path(
    case: &Dict,
    legal: &LegalMap<'_>,
    paths: &CartPaths,
) -> Option<(Variant, Variant)> {
    let input = dict(field(case, "input"));
    let recorded = field(case, "output");
    let call = label(case, "call");
    let answer = match call {
        "abnormal_condition.touches" | "abnormal_condition.interferes" => {
            let mut condition = read_condition(field(input, "self"));
            if !paths.substitute(&mut condition) {
                return None;
            }
            let point = point2(field(input, "point"));
            let radius = real(field(input, "radius"));
            if call == "abnormal_condition.touches" {
                condition.touches(point, radius)
            } else {
                let end = point2(field(input, "line_to"));
                let end = (end.x.is_finite() && end.y.is_finite()).then_some(end);
                condition.interferes(point, &outline(field(input, "offsets")), radius, end)
            }
        }
        "complete_relief.nearest" => {
            let mut area = read_relief(field(input, "area"));
            if !paths.substitute_area(&mut area) {
                return None;
            }
            let no_point = Vec3::new(Real::INFINITY, Real::INFINITY, Real::INFINITY);
            let nearest = relief::nearest(point3(field(input, "origin")), &area, legal);
            return Some((
                position(recorded.clone()),
                position(point_value(nearest.unwrap_or(no_point))),
            ));
        }
        "relief_area.allows" | "relief_area.accepts_first" | "relief_area.accepts_rest" => {
            let mut area = read_relief(field(input, "self"));
            if !paths.substitute_area(&mut area) {
                return None;
            }
            let point = point3(field(input, "point"));
            match call {
                "relief_area.allows" => area.allows(point, legal),
                "relief_area.accepts_first" => area.accepts_first(point, legal),
                _ => area.accepts_rest(point, point3(field(input, "first")), legal),
            }
        }
        _ => return None,
    };
    Some((recorded.clone(), Variant::Bool(answer)))
}

struct FixtureWorld<'a> {
    setup: HoleSetup,
    world: World<'a>,
    puddles: Vec<Vec<Vec2>>,
    repairs: Vec<Vec<Vec2>>,
}

impl<'a> FixtureWorld<'a> {
    fn read(input: &Dict, course: &'a Course) -> Result<Self, String> {
        let world = input
            .get("world")
            .and_then(Variant::as_dict)
            .ok_or("missing world descriptor")?;
        let identity = world
            .get("course")
            .and_then(Variant::as_dict)
            .ok_or("missing world course identity")?;
        let id = label(identity, "id");
        let revision = label(identity, "revision");
        if id != course.data().id || revision != course.revision() {
            return Err(format!("course {id} revision {revision} is unavailable"));
        }
        let recorded_setup = world
            .get("setup")
            .and_then(Variant::as_dict)
            .ok_or("missing hole setup")?;
        // The fixture stores the course identity alongside its setup.
        let mut fields = recorded_setup.entries().to_vec();
        if recorded_setup.get("course_id").is_none() {
            fields.push(("course_id".into(), id.into()));
        }
        let setup =
            HoleSetup::from_dict(&Dict::new(fields), revision.into(), PlaySettings::default())
                .map_err(|error| error.to_string())?;
        let clock_s = real(field(world, "clock_s"));
        let layers = |key| {
            items(field(world, key))
                .iter()
                .map(|mark| outline(field(dict(mark), "outline")))
                .collect()
        };
        let (world, puddles, repairs) = match label(world, "kind") {
            "standard" => (
                setup
                    .world(course, clock_s)
                    .ok_or("setup does not resolve a tee and cup on the recorded course")?,
                Vec::new(),
                Vec::new(),
            ),
            "generated" => {
                // Drop spots only sample height, which the generated ground takes from the course.
                let ground =
                    CourseGround::new(course, setup.firmness, setup.moisture, setup.green_speed_m);
                let cup = point3(field(world, "cup"));
                (
                    World::on_course(ground, setup.air(), Some(cup), clock_s),
                    layers("puddles"),
                    layers("repair_areas"),
                )
            }
            kind => return Err(format!("unknown world kind {kind:?}")),
        };
        Ok(Self {
            setup,
            world,
            puddles,
            repairs,
        })
    }

    fn marked(&self) -> ReliefOutlines<'_> {
        ReliefOutlines {
            temporary_water: &self.puddles,
            ground_under_repair: &self.repairs,
            ..ReliefOutlines::default()
        }
    }
}

#[derive(Default)]
struct CartPaths {
    outlines: Vec<(Vec<Vec2>, Vec<Vec2>)>,
    differences: usize,
}

impl CartPaths {
    fn compare(
        &mut self,
        id: &str,
        index: usize,
        point: Vec2,
        expected: &mut [Vec2],
        actual: &mut [Vec2],
    ) {
        if expected != actual {
            let delta = signed_area(actual).abs() - signed_area(expected).abs();
            println!(
                "{id} outline {}: cart-path area difference {delta:+.6} m²",
                index + 1
            );
            self.differences += 1;
        }
        assert_eq!(
            is_point_in_polygon(point, expected),
            is_point_in_polygon(point, actual),
            "{id}: point in cart-path outline"
        );
        if !self
            .outlines
            .iter()
            .any(|(recorded, _)| recorded == expected)
        {
            self.outlines.push((expected.to_vec(), actual.to_vec()));
        }
        canonicalize(expected);
        canonicalize(actual);
    }

    fn substitute_area(&self, area: &mut ReliefArea) -> bool {
        area.condition
            .as_mut()
            .is_some_and(|condition| self.substitute(condition))
    }

    fn substitute(&self, condition: &mut AbnormalCondition) -> bool {
        if condition.kind != AbnormalKind::ImmovableObstruction {
            return false;
        }
        let Some((_, computed)) = self
            .outlines
            .iter()
            .find(|(recorded, _)| *recorded == condition.outline)
        else {
            return false;
        };
        condition.outline.clone_from(computed);
        true
    }
}

fn signed_area(polygon: &[Vec2]) -> Real {
    polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1))
        .map(|(&a, &b)| a.cross(b))
        .sum::<Real>()
        * 0.5
}

fn canonicalize(polygon: &mut [Vec2]) {
    if signed_area(polygon) < 0.0 {
        polygon.reverse();
    }
    if let Some((start, _)) = polygon
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)))
    {
        polygon.rotate_left(start);
    }
}

#[derive(Default)]
struct Comparisons {
    cases: Vec<Variant>,
    actual: BTreeMap<String, Variant>,
}

impl Comparisons {
    fn add(&mut self, case: &Dict, expected: Variant, actual: Variant) {
        let id = label(case, "id");
        self.cases.push(object([
            ("id", field(case, "id").clone()),
            ("call", field(case, "call").clone()),
            ("input", field(case, "input").clone()),
            ("output", expected),
        ]));
        assert!(self.actual.insert(id.into(), actual).is_none());
    }

    fn check(self, testdata: &Path, source: &Dict) {
        // The shared corpus has a rules layer header. Compare this subset under relief.
        let scratch = Scratch::new(testdata);
        let path = scratch.0.join("testdata/fixture.bin");
        let count = self.cases.len();
        let fixture = object([
            ("format", field(source, "format").clone()),
            ("layer", "relief".into()),
            ("oracle", field(source, "oracle").clone()),
            ("cases", array(self.cases)),
        ]);
        let body = variant::encode(&fixture).unwrap();
        let mut bytes = b"GSCORP01".to_vec();
        bytes.extend(u32::try_from(body.len()).unwrap().to_le_bytes());
        bytes.extend(body);
        fs::write(&path, bytes).unwrap();

        let report = tolerance::check(&path, "relief", |case| {
            Ok::<_, String>(self.actual[label(case, "id")].clone())
        })
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(report.checked, count);
        assert!(report.discrete.is_empty());
        assert!(report.skipped.is_empty());
        print!("{report}");
    }
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(testdata: &Path) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("golf-relief-ground-{}-{now}", std::process::id()));
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

fn read_relief(value: &Variant) -> ReliefArea {
    use ReliefOption::*;
    let options = [
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
    ];
    let areas = [
        Area::General,
        Area::Teeing,
        Area::Green,
        Area::Bunker,
        Area::Penalty,
    ];
    let get = |key| field(dict(value), key);
    ReliefArea {
        option: options[usize::try_from(integer(get("option"))).unwrap()],
        reference: point3(get("reference")),
        hole: point3(get("hole")),
        club_length_m: real(get("club_length_m")),
        bunker: outline(get("bunker")),
        penalty_area: get("penalty_area")
            .as_dict()
            .map(|penalty| PenaltyAreaValue {
                outline: outline(field(penalty, "outline")),
                color: if boolean(field(penalty, "red")) {
                    PenaltyColor::Red
                } else {
                    PenaltyColor::Yellow
                },
            }),
        tee: outline(get("tee")),
        green: outline(get("green")),
        required_area: usize::try_from(integer(get("required_area")))
            .ok()
            .map(|index| areas[index]),
        green_placement: boolean(get("green_placement")),
        condition: (!matches!(get("condition"), Variant::Nil))
            .then(|| read_condition(get("condition"))),
        footprint: outline(get("footprint")),
        ball_radius_m: real(get("ball_radius_m")),
    }
}

fn read_condition(value: &Variant) -> AbnormalCondition {
    let condition = dict(value);
    let label = label(condition, "name");
    let kind = [
        AbnormalKind::TemporaryWater,
        AbnormalKind::GroundUnderRepair,
        AbnormalKind::ImmovableObstruction,
    ]
    .into_iter()
    .find(|kind| kind.name() == label)
    .unwrap_or_else(|| panic!("unknown abnormal condition {label}"));
    AbnormalCondition::new(kind, outline(field(condition, "outline")))
}

fn outline(value: &Variant) -> Vec<Vec2> {
    let Variant::PackedVector2Array(points) = value else {
        panic!("expected a packed Vector2 array")
    };
    points
        .values
        .iter()
        .map(|&[x, y]| Vec2::new(x, y))
        .collect()
}

fn outline_positions<'a>(outlines: impl Iterator<Item = &'a [Vec2]>) -> Variant {
    position(array(outlines.map(outline_value).collect()))
}

fn outline_value(points: &[Vec2]) -> Variant {
    Variant::PackedVector2Array(double(
        points.iter().map(|point| [point.x, point.y]).collect(),
    ))
}

fn spots_value(points: Vec<Vec3>) -> Variant {
    Variant::PackedVector3Array(double(
        points
            .iter()
            .map(|point| [point.x, point.y, point.z])
            .collect(),
    ))
}

fn point_value(point: Vec3) -> Variant {
    Variant::Vector3(double([point.x, point.y, point.z]))
}

fn point2(value: &Variant) -> Vec2 {
    let Variant::Vector2(point) = value else {
        panic!("expected a Vector2")
    };
    let [x, y] = point.values;
    Vec2::new(x, y)
}

fn point3(value: &Variant) -> Vec3 {
    let Variant::Vector3(point) = value else {
        panic!("expected a Vector3")
    };
    let [x, y, z] = point.values;
    Vec3::new(x, y, z)
}

fn double<T>(values: T) -> Components<T> {
    Components {
        values,
        width: FloatWidth::Double,
    }
}

fn position(value: Variant) -> Variant {
    object([("position", value)])
}

fn array(items: Vec<Variant>) -> Variant {
    Variant::Array(Array::new(items))
}

fn items(value: &Variant) -> &[Variant] {
    let Variant::Array(array) = value else {
        panic!("expected an Array")
    };
    &array.items
}

fn dict(value: &Variant) -> &Dict {
    value.as_dict().expect("expected a Dictionary")
}

fn label<'a>(dict: &'a Dict, name: &str) -> &'a str {
    field(dict, name)
        .as_str()
        .unwrap_or_else(|| panic!("{name}: expected a String"))
}

fn field<'a>(dict: &'a Dict, name: &str) -> &'a Variant {
    dict.get(name).unwrap_or_else(|| panic!("missing {name}"))
}

fn real(value: &Variant) -> Real {
    match value {
        Variant::Float(value) => value.value(),
        Variant::Int(value) => value.value() as Real,
        _ => panic!("expected a number"),
    }
}

fn integer(value: &Variant) -> i64 {
    let Variant::Int(value) = value else {
        panic!("expected an int")
    };
    value.value()
}

fn boolean(value: &Variant) -> bool {
    let Variant::Bool(value) = value else {
        panic!("expected a bool")
    };
    *value
}
