use std::path::Path;

use golf_sim_base::{
    Real, Vec2,
    corpus::{self, Tolerance},
    geometry::{
        get_closest_point_to_segment, is_point_in_polygon, segment_intersects_segment,
        union_cell_runs,
    },
    variant::{Components, FloatWidth, Variant},
};

const EDGE_TOLERANCE_M: Real = 1e-6;
const REFERENCE_POSITION_EPSILON: Real = 4.0 * f32::EPSILON as Real;

fn point(x: Real, y: Real) -> Vec2 {
    Vec2 { x, y }
}

fn as_point(value: &Variant) -> Vec2 {
    let Variant::Vector2(vector) = value else {
        panic!("expected a Vector2");
    };
    point(vector.values[0], vector.values[1])
}

fn as_variant(vector: Vec2) -> Variant {
    Variant::Vector2(Components {
        values: [vector.x, vector.y],
        width: FloatWidth::Double,
    })
}

fn distance_squared(a: Vec2, b: Vec2) -> Real {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    dx * dx + dy * dy
}

fn position_tolerance(inputs: &[Vec2]) -> Real {
    let scale = inputs
        .iter()
        .fold(1.0 as Real, |scale, p| scale.max(p.x.abs()).max(p.y.abs()));
    REFERENCE_POSITION_EPSILON * scale
}

fn crossing_near_endpoint(crossing: Vec2, a: Vec2, b: Vec2, tolerance: Real) -> bool {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let length_squared = dx * dx + dy * dy;
    let projection = (crossing.x - a.x) * dx + (crossing.y - a.y) * dy;
    let endpoint_delta = projection.abs().min((projection - length_squared).abs());
    // Compare |t - endpoint| <= tolerance / length without taking a square root.
    length_squared > 0.0
        && endpoint_delta * endpoint_delta <= tolerance * tolerance * length_squared
}

fn distance_for_report(squared: Real) -> Real {
    // The shared maths module is still a stub. Assertions use squared metres.
    let mut low = 0.0;
    let mut high = squared.max(1.0);
    for _ in 0..64 {
        let mid = (low + high) * 0.5;
        if mid * mid <= squared {
            low = mid;
        } else {
            high = mid;
        }
    }
    low
}

fn near_edge(query: Vec2, polygon: &[Vec2]) -> bool {
    polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1))
        .any(|(&a, &b)| {
            distance_squared(query, get_closest_point_to_segment(query, a, b))
                <= EDGE_TOLERANCE_M * EDGE_TOLERANCE_M
        })
}

#[test]
fn polygon_edge_crossing_uses_relative_approximation() {
    let square = [
        point(10.0, 10.0),
        point(12.0, 10.0),
        point(12.0, 12.0),
        point(10.0, 12.0),
    ];
    assert!(is_point_in_polygon(point(9.99995, 11.0), &square));
    assert!(!is_point_in_polygon(point(9.9998, 11.0), &square));
}

#[test]
fn geometry_matches_reference_cases() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/geometry/cases/fixture.bin");
    let fixture = corpus::read(&path).unwrap();
    let fixture = fixture.as_dict().unwrap();
    assert_eq!(
        fixture.get("layer").unwrap().as_str(),
        Some("geometry/cases")
    );
    let Variant::Array(cases) = fixture.get("cases").unwrap() else {
        panic!("expected fixture cases");
    };

    let mut counts = [0; 3];
    let mut near_edge_differences = Vec::new();
    let mut near_endpoint_differences = Vec::new();
    let mut failures = [0; 3];
    let mut largest_error_squared = [0.0 as Real; 3];
    let mut first_failure = [None, None, None];
    for case in &cases.items {
        let case = case.as_dict().unwrap();
        let field = |key| case.get(key).unwrap();
        let id = field("id").as_str().unwrap();
        let vector = |key| as_point(field(key));
        let expected = field("out");
        let (operation, actual, tolerance) = match field("op").as_str().unwrap() {
            "is_point_in_polygon" => {
                counts[0] += 1;
                let Variant::PackedVector2Array(vertices) = field("polygon") else {
                    panic!("{id} expected a PackedVector2Array");
                };
                let polygon: Vec<_> = vertices.values.iter().map(|&[x, y]| point(x, y)).collect();
                let query = vector("point");
                let actual = Variant::Bool(is_point_in_polygon(query, &polygon));
                if &actual != expected && near_edge(query, &polygon) {
                    near_edge_differences.push(id);
                    continue;
                }
                (0, actual, 0.0)
            }
            "get_closest_point_to_segment" => {
                counts[1] += 1;
                let [query, a, b] = ["point", "a", "b"].map(vector);
                let actual = get_closest_point_to_segment(query, a, b);
                (1, as_variant(actual), position_tolerance(&[query, a, b]))
            }
            "segment_intersects_segment" => {
                counts[2] += 1;
                let [a, b, c, d] = ["a", "b", "c", "d"].map(vector);
                let tolerance = position_tolerance(&[a, b, c, d]);
                let actual = segment_intersects_segment(a, b, c, d);
                let disputed_crossing = match (actual, expected) {
                    (Some(crossing), Variant::Nil) => Some(crossing),
                    (None, Variant::Vector2(_)) => Some(as_point(expected)),
                    _ => None,
                };
                if let Some(crossing) = disputed_crossing
                    && (crossing_near_endpoint(crossing, a, b, tolerance)
                        || crossing_near_endpoint(crossing, c, d, tolerance))
                {
                    near_endpoint_differences.push(id);
                    continue;
                }
                (2, actual.map_or(Variant::Nil, as_variant), tolerance)
            }
            operation => panic!("{id} unknown geometry operation {operation}"),
        };
        let difference = corpus::first_difference_with(expected, &actual, |_| {
            Some(Tolerance {
                absolute: tolerance,
                relative: 0.0,
            })
        });
        let farther_than_tolerance = if matches!(
            (&actual, expected),
            (Variant::Vector2(_), Variant::Vector2(_))
        ) {
            let error_squared = distance_squared(as_point(&actual), as_point(expected));
            largest_error_squared[operation] = largest_error_squared[operation].max(error_squared);
            error_squared > tolerance * tolerance
        } else {
            false
        };
        if difference.is_some() || farther_than_tolerance {
            failures[operation] += 1;
            first_failure[operation].get_or_insert_with(|| {
                format!("{id} at {difference:?}: expected {expected:?}, got {actual:?}")
            });
        }
    }

    assert!(counts.iter().all(|&count| count > 0));
    println!(
        "Checked {} polygon, {} closest-point and {} segment-crossing cases.",
        counts[0], counts[1], counts[2]
    );
    println!(
        "{} polygon differences within {EDGE_TOLERANCE_M} m of an edge: {}",
        near_edge_differences.len(),
        near_edge_differences.join(", ")
    );
    println!(
        "{} crossing differences within endpoint tolerance: {}",
        near_endpoint_differences.len(),
        near_endpoint_differences.join(", ")
    );
    println!(
        "Worst position errors: {:.9e} m for closest points, {:.9e} m for crossings.",
        distance_for_report(largest_error_squared[1]),
        distance_for_report(largest_error_squared[2])
    );
    println!(
        "{} boundary differences listed in total.",
        near_edge_differences.len() + near_endpoint_differences.len()
    );
    assert_eq!(
        failures, [0; 3],
        "cases outside tolerance by operation; first differences: {first_failure:?}"
    );
}

#[test]
fn corner_touching_runs_have_separate_canonical_outlines() {
    let runs = [
        (point(1.0, 1.0), point(2.0, 2.0)),
        (point(0.0, 0.0), point(1.0, 1.0)),
    ];
    assert_eq!(
        union_cell_runs(&runs),
        vec![
            vec![
                point(0.0, 0.0),
                point(1.0, 0.0),
                point(1.0, 1.0),
                point(0.0, 1.0),
            ],
            vec![
                point(1.0, 1.0),
                point(2.0, 1.0),
                point(2.0, 2.0),
                point(1.0, 2.0),
            ],
        ]
    );
}

const GRID: usize = 22;

fn quarter_cell(x: usize, z: usize) -> Vec2 {
    point((x as Real + 0.5) / 4.0, (z as Real + 0.5) / 4.0)
}

fn outside_cells(covered: &[[bool; GRID]; GRID]) -> [[bool; GRID]; GRID] {
    let mut outside = [[false; GRID]; GRID];
    let mut pending = vec![(0usize, 0usize)];
    outside[0][0] = true;
    while let Some((x, z)) = pending.pop() {
        // Empty cells can connect diagonally between two corner-touching outlines.
        for nz in z.saturating_sub(1)..=(z + 1).min(GRID - 1) {
            for nx in x.saturating_sub(1)..=(x + 1).min(GRID - 1) {
                if !covered[nz][nx] && !outside[nz][nx] {
                    outside[nz][nx] = true;
                    pending.push((nx, nz));
                }
            }
        }
    }
    outside
}

fn contains_cell(outline: &[Vec2], cell: Vec2) -> bool {
    let crossings = outline
        .iter()
        .zip(outline.iter().cycle().skip(1))
        .filter(|(a, b)| (a.y > cell.y) != (b.y > cell.y) && a.x > cell.x)
        .count();
    crossings % 2 == 1
}

fn check_cell_union(runs: &[(Vec2, Vec2)]) -> bool {
    let outlines = union_cell_runs(runs);
    let mut reversed = runs.to_vec();
    reversed.reverse();
    assert_eq!(outlines, union_cell_runs(&reversed));

    let vertex_order = |p: Vec2| (p.y, p.x);
    assert!(
        outlines
            .windows(2)
            .all(|pair| vertex_order(pair[0][0]) < vertex_order(pair[1][0])),
        "{runs:?}"
    );
    for outline in &outlines {
        assert!(outline.len() >= 4, "{outline:?}");
        assert!(outline[1].x > outline[0].x && outline[1].y == outline[0].y);
        for (i, &vertex) in outline.iter().enumerate() {
            let next = outline[(i + 1) % outline.len()];
            let after = outline[(i + 2) % outline.len()];
            assert!(vertex_order(outline[0]) <= vertex_order(vertex));
            assert_ne!(vertex.x == next.x, vertex.y == next.y, "{outline:?}");
            assert_ne!(vertex.x == next.x, next.x == after.x, "{outline:?}");
            assert!(
                runs.iter()
                    .any(|(low, high)| vertex.x == low.x || vertex.x == high.x)
            );
            assert!(
                runs.iter()
                    .any(|(low, high)| vertex.y == low.y || vertex.y == high.y)
            );
        }
    }

    let covered = std::array::from_fn(|z| {
        std::array::from_fn(|x| {
            let p = quarter_cell(x, z);
            runs.iter()
                .any(|(low, high)| low.x < p.x && p.x < high.x && low.y < p.y && p.y < high.y)
        })
    });
    let outside = outside_cells(&covered);
    let mut has_hole = false;
    for z in 0..GRID {
        for x in 0..GRID {
            let p = quarter_cell(x, z);
            let count = outlines
                .iter()
                .filter(|outline| contains_cell(outline, p))
                .count();
            assert_eq!(count, usize::from(!outside[z][x]), "{p:?} in {runs:?}");
            has_hole |= !outside[z][x] && !covered[z][x];
        }
    }
    has_hole
}

fn unit_cells(size: u32, mask: u32) -> Vec<(Vec2, Vec2)> {
    (0..size * size)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| {
            let x = Real::from(i % size + 1);
            let z = Real::from(i / size + 1);
            (point(x, z), point(x + 1.0, z + 1.0))
        })
        .collect()
}

#[test]
fn union_outlines_enclose_the_cells_and_fill_holes() {
    let mut with_holes = 0;
    for mask in 0..1 << 9 {
        with_holes += usize::from(check_cell_union(&unit_cells(3, mask)));
    }
    let mut state = 0x92a1_8743u32;
    for _ in 0..256 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let mut runs = unit_cells(4, state >> 16);
        let x = Real::from(state % 4) + 0.25;
        runs.extend([
            (point(x, 0.25), point(x + 1.5, 1.5)),
            (point(x + 0.25, 0.5), point(x + 0.75, 1.75)),
        ]);
        with_holes += usize::from(check_cell_union(&runs));
    }
    assert!((1..768).contains(&with_holes));
    println!(
        "Cell enclosure and lattice coordinates verified for 768 inputs ({with_holes} with holes)."
    );
}
