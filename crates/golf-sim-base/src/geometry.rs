//! Geometry on the ground plane. A vector's y component holds the ground's z coordinate.

use std::collections::BTreeSet;

use crate::{Real, Vec2};

const CMP_EPSILON: Real = 0.00001;

fn is_equal_approx(a: Real, b: Real) -> bool {
    a == b || (a - b).abs() < (CMP_EPSILON * a.abs()).max(CMP_EPSILON)
}

/// Tests the parity of the crossings from the point to a point beyond the polygon.
/// A crossing approximately equal to the query counts as inside.
pub fn is_point_in_polygon(point: Vec2, polygon: &[Vec2]) -> bool {
    if polygon.len() < 3 {
        return false;
    }

    let mut far = Vec2 { x: -1e20, y: -1e20 };
    let mut near = Vec2 { x: 1e20, y: 1e20 };
    for vertex in polygon {
        far.x = far.x.max(vertex.x);
        far.y = far.y.max(vertex.y);
        near.x = near.x.min(vertex.x);
        near.y = near.y.min(vertex.y);
    }
    far.x += (far.x - near.x) * 1.221313;
    far.y += (far.y - near.y) * 1.512312;

    let mut inside = false;
    for (&a, &b) in polygon.iter().zip(polygon.iter().cycle().skip(1)) {
        if let Some(crossing) = segment_intersects_segment(a, b, point, far) {
            if is_equal_approx(crossing.x, point.x) && is_equal_approx(crossing.y, point.y) {
                return true;
            }
            inside = !inside;
        }
    }
    inside
}

/// Projects a point onto a closed segment, returning its start for a degenerate segment.
pub fn get_closest_point_to_segment(point: Vec2, a: Vec2, b: Vec2) -> Vec2 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let length_squared = dx * dx + dy * dy;
    if length_squared < 1e-20 {
        return a;
    }

    let t = (dx * (point.x - a.x) + dy * (point.y - a.y)) / length_squared;
    if t <= 0.0 {
        a
    } else if t >= 1.0 {
        b
    } else {
        Vec2 {
            x: a.x + dx * t,
            y: a.y + dy * t,
        }
    }
}

/// Tests two closed segments for a crossing. Collinear segments have no unique crossing.
pub fn segment_intersects_segment(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> Option<Vec2> {
    let ab_x = b.x - a.x;
    let ab_y = b.y - a.y;
    let length_squared = ab_x * ab_x + ab_y * ab_y;
    if length_squared <= 0.0 {
        return None;
    }

    // Express C and D in coordinates where AB runs from (0, 0) to (1, 0).
    let nx = ab_x / length_squared;
    let ny = ab_y / length_squared;
    let ac_x = c.x - a.x;
    let ac_y = c.y - a.y;
    let ad_x = d.x - a.x;
    let ad_y = d.y - a.y;
    let c_across = ac_y * nx - ac_x * ny;
    let d_across = ad_y * nx - ad_x * ny;
    if (c_across < -CMP_EPSILON && d_across < -CMP_EPSILON)
        || (c_across > CMP_EPSILON && d_across > CMP_EPSILON)
        || is_equal_approx(c_across, d_across)
    {
        return None;
    }

    let c_along = ac_x * nx + ac_y * ny;
    let d_along = ad_x * nx + ad_y * ny;
    let t = d_along + (c_along - d_along) * d_across / (d_across - c_across);
    if (0.0..=1.0).contains(&t) {
        Some(Vec2 {
            x: a.x + ab_x * t,
            y: a.y + ab_y * t,
        })
    } else {
        None
    }
}

type Vertex = (usize, usize); // (z, x)
type Edge = (Vertex, Direction);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Direction {
    East,
    South,
    West,
    North,
}

impl Direction {
    fn advance(self, (z, x): Vertex) -> Vertex {
        match self {
            Self::East => (z, x + 1),
            Self::South => (z + 1, x),
            Self::West => (z, x - 1),
            Self::North => (z - 1, x),
        }
    }

    fn exits(self) -> [Self; 3] {
        const CLOCKWISE: [Direction; 4] = [
            Direction::East,
            Direction::South,
            Direction::West,
            Direction::North,
        ];
        let index = self as usize;
        [CLOCKWISE[(index + 1) % 4], self, CLOCKWISE[(index + 3) % 4]]
    }
}

/// Unions axis-aligned rectangles supplied as (low, high) corners.
/// Returns clockwise outer outlines in order of their least (z, x) vertex.
/// Each begins at that vertex heading +x, with no repeated closing vertex or
/// redundant collinear vertices. Corner-only contacts stay separate. Hole
/// boundaries are dropped, so an island in a hole yields a nested outer outline.
/// Rectangles without finite, positive extents are ignored.
pub fn union_cell_runs(rectangles: &[(Vec2, Vec2)]) -> Vec<Vec<Vec2>> {
    let rectangles: Vec<_> = rectangles
        .iter()
        .copied()
        .filter(|(low, high)| {
            low.x.is_finite()
                && low.y.is_finite()
                && high.x.is_finite()
                && high.y.is_finite()
                && low.x < high.x
                && low.y < high.y
        })
        .collect();
    if rectangles.is_empty() {
        return Vec::new();
    }

    let xs = lattice_axis(&rectangles, |point| point.x);
    let zs = lattice_axis(&rectangles, |point| point.y);
    let width = xs.len() - 1;
    let height = zs.len() - 1;
    let mut covered = vec![vec![false; width]; height];
    for (low, high) in rectangles {
        let x = axis_index(&xs, low.x)..axis_index(&xs, high.x);
        for row in &mut covered[axis_index(&zs, low.y)..axis_index(&zs, high.y)] {
            row[x.clone()].fill(true);
        }
    }

    let mut boundary = BTreeSet::new();
    for (z, row) in covered.iter().enumerate() {
        for (x, &is_covered) in row.iter().enumerate() {
            if !is_covered {
                continue;
            }
            if z == 0 || !covered[z - 1][x] {
                boundary.insert(((z, x), Direction::East));
            }
            if x + 1 == width || !row[x + 1] {
                boundary.insert(((z, x + 1), Direction::South));
            }
            if z + 1 == height || !covered[z + 1][x] {
                boundary.insert(((z + 1, x + 1), Direction::West));
            }
            if x == 0 || !row[x - 1] {
                boundary.insert(((z + 1, x), Direction::North));
            }
        }
    }

    let mut outlines = Vec::new();
    while let Some(&start) = boundary.first() {
        if let Some(corners) = walk_outline(&mut boundary, start) {
            outlines.push(
                corners
                    .into_iter()
                    .map(|(z, x)| Vec2 { x: xs[x], y: zs[z] })
                    .collect(),
            );
        }
    }
    outlines
}

fn lattice_axis(rectangles: &[(Vec2, Vec2)], component: impl Fn(Vec2) -> Real) -> Vec<Real> {
    let mut axis: Vec<_> = rectangles
        .iter()
        .flat_map(|&(low, high)| [component(low), component(high)])
        .collect();
    axis.sort_by(Real::total_cmp);
    axis.dedup();
    axis
}

fn axis_index(axis: &[Real], value: Real) -> usize {
    axis.binary_search_by(|probe| probe.partial_cmp(&value).unwrap())
        .unwrap()
}

fn walk_outline(boundary: &mut BTreeSet<Edge>, start: Edge) -> Option<Vec<Vertex>> {
    let mut edge = start;
    let mut corners = vec![start.0];
    let mut twice_area = 0;
    loop {
        boundary.remove(&edge);
        let ((z, x), direction) = edge;
        let (next_z, next_x) = direction.advance((z, x));
        // The signed area on lattice indices has the same sign as on the input coordinates.
        twice_area += x as i128 * next_z as i128 - next_x as i128 * z as i128;
        let next = direction
            .exits()
            .into_iter()
            .map(|exit| ((next_z, next_x), exit))
            .find(|next| *next == start || boundary.contains(next))
            .expect("a boundary edge always has a successor");
        if next == start {
            break;
        }
        if direction != next.1 {
            corners.push(next.0);
        }
        edge = next;
    }
    (twice_area > 0).then_some(corners)
}
