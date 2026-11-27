use golf_sim_base::{
    Real, Vec2,
    digest::object,
    variant::{Dict, Variant},
};
use golf_sim_rules::relief::{self, AbnormalCondition};

use super::{
    NO_POINT, condition_value, dict, double, field, integer, legal_course, outline, outline_value,
    point_value, point3, read_condition, read_kind, read_relief, real, replay,
};

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

use std::{collections::BTreeMap, sync::Arc};

use golf_sim_course::course::Course;

use super::ground::{CartPaths, FixtureWorld, replay_cart_path, replay_world};
use crate::corpus::{
    FixtureRun, Result,
    projection::{Pair, id, rows},
    stroke,
};

pub(in crate::corpus) fn project(
    fixture: &mut FixtureRun,
    header: &Dict,
    drift: bool,
) -> Result<Vec<Pair>> {
    let mut paths = BTreeMap::<_, (Arc<Course>, CartPaths)>::new();
    let mut world_outputs = BTreeMap::new();
    let world_cases = rows(header)
        .enumerate()
        .filter(|(_, case)| call(case).starts_with("relief_ground."));
    let (drops, outlines): (Vec<_>, Vec<_>) =
        world_cases.partition(|(_, case)| call(case) == "relief_ground.drop_spots");
    for (index, case) in outlines.into_iter().chain(drops) {
        let course = fixture.require_course(case)?;
        let start = FixtureWorld::read(dict(field(case, "input")), &course)
            .map_err(|error| format!("{}: {error}", id(case)))?;
        let (_, paths) = paths
            .entry((course.data().id.clone(), course.revision().to_owned()))
            .or_insert_with(|| (Arc::clone(&course), CartPaths::default()));
        world_outputs.insert(index, replay_world(case, &start, &course, paths));
    }

    let mut dependent = 0;
    let pairs = rows(header)
        .enumerate()
        .map(|(index, case)| {
            if let Some(pair) = world_outputs.remove(&index) {
                return Ok(pair);
            }

            let output = field(case, "output");
            if matches!(call(case), "stroke.drop" | "stroke.place") {
                if uses_generated_stroke(case) {
                    return Ok((output.clone(), Variant::Nil));
                }
                let course = fixture.resolve_course(case)?;
                let actual = stroke::facade(case, course.as_deref(), drift)?;
                return Ok((stroke_positions(output), stroke_positions(&actual)));
            }

            let mut actual = run(call(case), dict(field(case, "input")));
            let positional = matches!(
                call(case),
                "complete_relief.nearest"
                    | "abnormal_condition.closest_edge"
                    | "relief_area.outline"
                    | "relief_area.flat"
            );
            let mut expected = if positional {
                object([("position", output.clone())])
            } else {
                output.clone()
            };
            if !drift && !super::integers_match(&expected, &actual) {
                return Err(format!("{}: integer output differs", id(case)).into());
            }
            let course = fixture.resolve_course(case)?;
            for (owner, paths) in paths.values().filter(|(owner, _)| {
                course
                    .as_ref()
                    .is_none_or(|course| Arc::ptr_eq(owner, course))
            }) {
                if let Some((reference, calculated)) = replay_cart_path(case, &owner.legal(), paths)
                {
                    dependent += 1;
                    let join_fields = |plain, extra| {
                        if positional {
                            object([(
                                "position",
                                super::array([
                                    field(dict(&plain), "position").clone(),
                                    field(dict(&extra), "position").clone(),
                                ]),
                            )])
                        } else {
                            object([("recorded_outline", plain), ("computed_outline", extra)])
                        }
                    };
                    expected = join_fields(expected, reference);
                    actual = join_fields(actual, calculated);
                    break;
                }
            }
            Ok((expected, actual))
        })
        .collect::<Result<Vec<_>>>()?;
    if dependent != 0 {
        println!("Compared {dependent} cases with computed cart-path outlines.");
    }
    Ok(pairs)
}

fn call(case: &Dict) -> &str {
    field(case, "call").as_str().unwrap()
}

fn stroke_positions(output: &Variant) -> Variant {
    let (positions, fields): (Vec<_>, Vec<_>) = dict(output)
        .entries()
        .iter()
        .cloned()
        .partition(|(key, _)| matches!(key.as_str(), Some("first" | "rest" | "spot")));
    Variant::Dict(Dict::new(
        [("position".into(), Variant::Dict(Dict::new(positions)))]
            .into_iter()
            .chain(fields)
            .collect(),
    ))
}

pub(in crate::corpus) fn uses_generated_stroke(case: &Dict) -> bool {
    matches!(call(case), "stroke.drop" | "stroke.place")
        && crate::corpus::child(dict(field(case, "input")), "world")
            .and_then(|world| world.get("kind"))
            .and_then(Variant::as_str)
            == Some("generated")
}
