//! Abnormal ground, tee boundaries and selectable drop spots for a hole.

use std::f64::consts::TAU;

use golf_sim_base::{Real, Surface, Vec2, Vec3, fm, geometry::union_cell_runs};
use golf_sim_conditions::world::World;
use golf_sim_course::course::{Course, LegalKind};
use golf_sim_physics::samplers::GroundSampler;
use golf_sim_rules::relief::{AbnormalCondition, AbnormalKind, ReliefArea, flat};

use crate::setup::HoleSetup;

const PATH_SEARCH_M: Real = 8.0;
const LINE_MAX_M: Real = 100.0;

/// Marked outlines supplied by the current round moment and course setup.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReliefOutlines<'a> {
    pub temporary_water: &'a [Vec<Vec2>],
    pub ground_under_repair: &'a [Vec<Vec2>],
    pub immovable_obstructions: &'a [Vec<Vec2>],
}

pub fn tee_outline<'a>(setup: &HoleSetup, course: &'a Course) -> &'a [Vec2] {
    let id = format!("h{}/tee/{}", setup.hole, setup.tee_id);
    course
        .data()
        .regions
        .iter()
        .find(|region| region.legal == LegalKind::TeeingArea && region.id == id)
        .map_or(&[], |region| region.polygon.as_slice())
}

pub fn conditions(
    course: &Course,
    point: Vec3,
    marked: ReliefOutlines<'_>,
) -> Vec<AbnormalCondition> {
    [
        (AbnormalKind::TemporaryWater, marked.temporary_water),
        (AbnormalKind::GroundUnderRepair, marked.ground_under_repair),
        (
            AbnormalKind::ImmovableObstruction,
            marked.immovable_obstructions,
        ),
    ]
    .into_iter()
    .flat_map(|(kind, polygons)| {
        polygons
            .iter()
            .cloned()
            .map(move |outline| AbnormalCondition::new(kind, outline))
    })
    .chain(
        cart_path_outlines(course, flat(point))
            .into_iter()
            .map(|outline| AbnormalCondition::new(AbnormalKind::ImmovableObstruction, outline)),
    )
    .collect()
}

pub fn cart_path_outlines(course: &Course, point: Vec2) -> Vec<Vec<Vec2>> {
    let data = course.data();
    if !data
        .materials
        .iter()
        .any(|material| material.kind == Surface::CartPath)
    {
        return Vec::new();
    }

    let margin = Vec2::new(PATH_SEARCH_M, PATH_SEARCH_M);
    let search_low = point - margin;
    let search_high = Vec2::new(point.x + margin.x, point.y + margin.y);
    let mut runs = Vec::new();
    for grid in &data.surface_tiles {
        let frame = grid.frame();
        let origin = frame.origin;
        let end = frame.span_end();
        if search_low.x >= end.x
            || search_low.y >= end.y
            || search_high.x <= origin.x
            || search_high.y <= origin.y
        {
            continue;
        }
        let low = (search_low - origin) / frame.cell_m;
        let high = (search_high - origin) / frame.cell_m;
        let [width, height] = frame.size;
        let columns = fm::floor(low.x) as usize..(fm::ceil(high.x) as usize).min(width);
        let rows = fm::floor(low.y) as usize..(fm::ceil(high.y) as usize).min(height);
        let node = |column, row| {
            Vec2::new(
                origin.x + column * frame.cell_m,
                origin.y + row * frame.cell_m,
            )
        };
        for row in rows {
            let mut first = None;
            for column in columns.start..=columns.end {
                let is_path = column < columns.end && {
                    let index = grid.material_at(node(column as Real, row as Real)).0;
                    data.materials[usize::from(index)].kind == Surface::CartPath
                };
                if is_path {
                    first.get_or_insert(column);
                } else if let Some(start) = first.take() {
                    // Material samples are cell centres, so edges sit half a cell away.
                    runs.push((
                        node(start as Real - 0.5, row as Real - 0.5),
                        node(column as Real - 0.5, row as Real + 0.5),
                    ));
                }
            }
        }
    }
    union_cell_runs(&runs)
}

pub fn drop_spots(area: &ReliefArea, world: &World<'_>, course: &Course) -> Vec<Vec3> {
    let Some(legal) = world.legal() else {
        return Vec::new();
    };
    let mut spots = Vec::new();
    let back = area.back();
    let point_at = |offset: Vec2| area.reference + Vec3::new(offset.x, 0.0, offset.y);
    let add = |spots: &mut Vec<Vec3>, point: Vec3| {
        if area.accepts_first(point, legal) {
            let sample = world.ground().sample_ground(point.x, point.z);
            let spot = Vec3::new(point.x, sample.height_m, point.z);
            if !spots.contains(&spot) {
                spots.push(spot);
            }
        }
    };

    if area.on_line() {
        let reach = line_reach(flat(area.reference), course).min(LINE_MAX_M);
        add(&mut spots, point_at(back * 0.02));
        let mut distance = 1.0;
        while distance <= reach {
            let point = point_at(back * distance);
            if distance > 1.0 && legal.out_of_bounds(point, 0.0) {
                break;
            }
            add(&mut spots, point);
            distance += if distance < 4.0 {
                1.0
            } else if distance < 10.0 {
                2.0
            } else {
                5.0
            };
        }
    } else {
        let reach = area.reach_m();
        for fraction in [0.88, 0.45, 0.0] {
            for i in 0..12 {
                let offset = back.rotated(TAU * Real::from(i) / 12.0) * reach * fraction;
                add(&mut spots, point_at(offset));
            }
            if !spots.is_empty() {
                break;
            }
        }
    }
    spots
}

fn line_reach(start: Vec2, course: &Course) -> Real {
    let data = course.data();
    data.surface_tiles
        .iter()
        .chain(data.property.iter().map(|property| &property.surface))
        .flat_map(|tile| {
            let low = tile.frame().origin;
            let high = tile.frame().span_end();
            [
                low,
                high,
                Vec2::new(low.x, high.y),
                Vec2::new(high.x, low.y),
            ]
        })
        .map(|corner| start.distance_to(corner))
        .fold(0.0, Real::max)
}
