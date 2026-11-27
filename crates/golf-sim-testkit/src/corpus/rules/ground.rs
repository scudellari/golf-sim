use golf_sim::{
    hole::relief_ground::{self, ReliefOutlines},
    setup::{HoleSetup, PlaySettings},
};
use golf_sim_base::{
    Real, Vec2, Vec3,
    digest::object,
    geometry::is_point_in_polygon,
    variant::{Dict, Variant},
};
use golf_sim_conditions::{grounds::CourseGround, world::World};
use golf_sim_course::{course::Course, legal::LegalMap};
use golf_sim_rules::relief::{
    self, AbnormalCondition, AbnormalKind, ReliefArea, ReliefOption, flat,
};

use super::{
    array, dict, double, field, items, outline, outline_value, point_value, point3, read_condition,
    read_relief,
};

pub(super) fn replay_world(
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
                paths.substitute_area(&mut area);
            }
            let spots = relief_ground::drop_spots(&area, &start.world, course);
            (position(recorded.clone()), position(spots_value(spots)))
        }
        call => panic!("{id}: unexpected relief ground call {call}"),
    }
}

pub(super) fn replay_cart_path(
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

pub(super) struct FixtureWorld<'a> {
    setup: HoleSetup,
    world: World<'a>,
    puddles: Vec<Vec<Vec2>>,
    repairs: Vec<Vec<Vec2>>,
}

impl<'a> FixtureWorld<'a> {
    pub(super) fn read(input: &Dict, course: &'a Course) -> Result<Self, String> {
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
pub(super) struct CartPaths {
    outlines: Vec<(Vec<Vec2>, Vec<Vec2>)>,
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
                "{id} outline {}: cart-path vertices differ (area difference {delta:+.6} m²)",
                index + 1
            );
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

fn outline_positions<'a>(outlines: impl Iterator<Item = &'a [Vec2]>) -> Variant {
    position(array(outlines.map(outline_value)))
}

fn spots_value(points: Vec<Vec3>) -> Variant {
    Variant::PackedVector3Array(double(
        points
            .iter()
            .map(|point| [point.x, point.y, point.z])
            .collect(),
    ))
}

fn point2(value: &Variant) -> Vec2 {
    let Variant::Vector2(point) = value else {
        panic!("expected a Vector2")
    };
    let [x, y] = point.values;
    Vec2::new(x, y)
}

fn position(value: Variant) -> Variant {
    object([("position", value)])
}

fn label<'a>(dict: &'a Dict, name: &str) -> &'a str {
    field(dict, name)
        .as_str()
        .unwrap_or_else(|| panic!("{name}: expected a String"))
}

fn real(value: &Variant) -> Real {
    match value {
        Variant::Float(value) => value.value(),
        Variant::Int(value) => value.value() as Real,
        _ => panic!("expected a number"),
    }
}
