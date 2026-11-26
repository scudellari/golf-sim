use golf_sim_base::{Real, Vec2, Vec3};
use golf_sim_physics::{samplers::ZoneCrossing, values::Zone};

use crate::course::{Course, CourseData, LegalKind};

const MERGE_M: Real = 1e-6;
const CORNER_SLACK: Real = 1e-9;

#[derive(Debug)]
pub struct ZoneMap<'a> {
    spans: Vec<[Vec2; 4]>,
    regions: Vec<(Zone, Bounds, &'a [Vec2])>,
    edges: Vec<(Vec2, Vec2)>,
    index: SpatialIndex,
}

impl Course {
    pub fn zone_map(&self) -> ZoneMap<'_> {
        ZoneMap::new(self.data())
    }
}

impl<'a> ZoneMap<'a> {
    pub fn new(course: &'a CourseData) -> Self {
        let spans: Vec<_> = surface_outlines(course).collect();
        let regions: Vec<_> = [
            (LegalKind::PenaltyArea, Zone::PenaltyArea),
            (LegalKind::OutOfBounds, Zone::OutOfBounds),
        ]
        .into_iter()
        .flat_map(|(kind, zone)| {
            course.regions.iter().filter_map(move |region| {
                if region.legal != kind {
                    return None;
                }
                let outline = region.polygon.as_slice();
                Some((zone, Bounds::of(outline), outline))
            })
        })
        .collect();
        let edges: Vec<_> = spans
            .iter()
            .map(<[_; 4]>::as_slice)
            .chain(regions.iter().map(|&(_, _, outline)| outline))
            .flat_map(polygon_edges)
            .collect();
        let index = SpatialIndex::new(edges.iter().map(|&(a, b)| Bounds::between(a, b)));
        Self {
            spans,
            regions,
            edges,
            index,
        }
    }

    pub fn zone_at(&self, position: Vec3) -> Zone {
        let p = plan(position);
        let point_bounds = Bounds::between(p, p);
        for &(zone, bounds, outline) in self.regions.iter().rev() {
            if bounds.overlaps(point_bounds) && contains(outline, p) {
                return zone;
            }
        }
        if self.spans.iter().any(|span| contains(span, p)) {
            return Zone::InPlay;
        }
        Zone::OutOfBounds
    }

    /// Replaces `out` with the zones entered at fractions in (0, 1].
    pub fn crossings(&self, from: Vec3, to: Vec3, from_zone: Zone, out: &mut Vec<ZoneCrossing>) {
        out.clear();
        let a = plan(from);
        let b = plan(to);
        let delta = b - a;
        let length = delta.length();
        if length == 0.0 || !length.is_finite() {
            return;
        }

        let mut hits = Vec::new();
        self.index.visit(Bounds::between(a, b).grow(MERGE_M), |i| {
            let (p, q) = self.edges[i];
            let edge = q - p;
            let denominator = delta.cross(edge);
            if denominator != 0.0 {
                let along_path = (p - a).cross(edge) / denominator;
                let along_edge = (p - a).cross(delta) / denominator;
                if along_path > 0.0
                    && along_path <= 1.0
                    && (-CORNER_SLACK..=1.0 + CORNER_SLACK).contains(&along_edge)
                {
                    hits.push(along_path);
                }
            }
        });
        hits.sort_unstable_by(Real::total_cmp);

        let mut zone = from_zone;
        let mut remaining = hits.as_slice();
        while let Some((&at, rest)) = remaining.split_first() {
            let merged = rest.partition_point(|next| (next - at) * length < MERGE_M);
            remaining = &rest[merged..];
            let until = remaining.first().copied().unwrap_or(1.0);
            // The segment ending on an edge owns the crossing into the zone just beyond it.
            let probe = if (until - at) * length >= MERGE_M {
                (at + until) * 0.5
            } else {
                at + MERGE_M / length
            };
            let entered = self.zone_at(world(a.lerp(b, probe), 0.0));
            if entered != zone {
                out.push(ZoneCrossing {
                    fraction: at,
                    zone: entered,
                });
                zone = entered;
            }
        }
    }
}

pub(crate) fn surface_outlines(course: &CourseData) -> impl Iterator<Item = [Vec2; 4]> + '_ {
    course
        .surface_tiles
        .iter()
        .chain(course.property.iter().map(|property| &property.surface))
        .map(|surface| {
            let a = surface.frame().origin;
            let b = surface.frame().span_end();
            [a, Vec2::new(b.x, a.y), b, Vec2::new(a.x, b.y)]
        })
}

pub(crate) fn polygon_edges(outline: &[Vec2]) -> impl Iterator<Item = (Vec2, Vec2)> + '_ {
    outline
        .last()
        .into_iter()
        .chain(outline)
        .copied()
        .zip(outline.iter().copied())
}

pub(crate) fn contains(outline: &[Vec2], p: Vec2) -> bool {
    let mut inside = false;
    for (previous, current) in polygon_edges(outline) {
        if (current.y > p.y) != (previous.y > p.y)
            && p.x
                < (previous.x - current.x) * (p.y - current.y) / (previous.y - current.y)
                    + current.x
        {
            inside = !inside;
        }
    }
    inside
}

pub(crate) fn plan(point: Vec3) -> Vec2 {
    Vec2::new(point.x, point.z)
}

pub(crate) fn world(point: Vec2, height: Real) -> Vec3 {
    Vec3::new(point.x, height, point.y)
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Bounds {
    min: Vec2,
    max: Vec2,
}

impl Bounds {
    pub(crate) fn between(a: Vec2, b: Vec2) -> Self {
        Self {
            min: Vec2::new(a.x.min(b.x), a.y.min(b.y)),
            max: Vec2::new(a.x.max(b.x), a.y.max(b.y)),
        }
    }

    pub(crate) fn of(points: &[Vec2]) -> Self {
        points.iter().fold(
            Self {
                min: Vec2::new(Real::INFINITY, Real::INFINITY),
                max: Vec2::new(Real::NEG_INFINITY, Real::NEG_INFINITY),
            },
            |bounds, &p| bounds.union(Self::between(p, p)),
        )
    }

    fn union(self, other: Self) -> Self {
        Self {
            min: Vec2::new(self.min.x.min(other.min.x), self.min.y.min(other.min.y)),
            max: Vec2::new(self.max.x.max(other.max.x), self.max.y.max(other.max.y)),
        }
    }

    pub(crate) fn grow(self, by: Real) -> Self {
        Self {
            min: Vec2::new(self.min.x - by, self.min.y - by),
            max: Vec2::new(self.max.x + by, self.max.y + by),
        }
    }

    fn overlaps(self, other: Self) -> bool {
        self.min.x <= other.max.x
            && self.max.x >= other.min.x
            && self.min.y <= other.max.y
            && self.max.y >= other.min.y
    }
}

#[derive(Debug)]
pub(crate) struct SpatialIndex(Option<SpatialNode>);

#[derive(Debug)]
struct SpatialNode {
    bounds: Bounds,
    children: SpatialChildren,
}

#[derive(Debug)]
enum SpatialChildren {
    Item(usize),
    Pair(Box<[SpatialNode; 2]>),
}

impl SpatialIndex {
    pub(crate) fn new(bounds: impl Iterator<Item = Bounds>) -> Self {
        let mut items: Vec<_> = bounds.enumerate().collect();
        Self((!items.is_empty()).then(|| SpatialNode::build(&mut items)))
    }

    pub(crate) fn visit(&self, bounds: Bounds, mut visit: impl FnMut(usize)) {
        if let Some(root) = &self.0 {
            root.visit(bounds, &mut visit);
        }
    }
}

impl SpatialNode {
    fn build(items: &mut [(usize, Bounds)]) -> Self {
        let bounds = items
            .iter()
            .map(|&(_, rect)| rect)
            .reduce(Bounds::union)
            .unwrap();
        if let [(item, _)] = items {
            return Self {
                bounds,
                children: SpatialChildren::Item(*item),
            };
        }
        let span = bounds.max - bounds.min;
        let centre = |rect: Bounds| {
            if span.x >= span.y {
                rect.min.x * 0.5 + rect.max.x * 0.5
            } else {
                rect.min.y * 0.5 + rect.max.y * 0.5
            }
        };
        let middle = items.len() / 2;
        items.select_nth_unstable_by(middle, |a, b| {
            centre(a.1).total_cmp(&centre(b.1)).then(a.0.cmp(&b.0))
        });
        let (left, right) = items.split_at_mut(middle);
        Self {
            bounds,
            children: SpatialChildren::Pair(Box::new([Self::build(left), Self::build(right)])),
        }
    }

    fn visit(&self, bounds: Bounds, visit: &mut impl FnMut(usize)) {
        if !self.bounds.overlaps(bounds) {
            return;
        }
        match &self.children {
            SpatialChildren::Item(item) => visit(*item),
            SpatialChildren::Pair(nodes) => {
                for node in nodes.iter() {
                    node.visit(bounds, visit);
                }
            }
        }
    }
}
