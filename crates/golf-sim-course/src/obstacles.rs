use golf_sim_base::{Basis, Real, Vec2, Vec3, fm};
use golf_sim_physics::values::{ObstacleHit, ObstacleKind};

use crate::{
    course::{Course, Rock, Tree, Wall},
    zones::{Bounds, SpatialIndex, plan, world},
};

const UP: Vec3 = Vec3::new(0.0, 1.0, 0.0);
const RAY_RESOLUTION_M: Real = 0.0000625;

#[derive(Debug)]
pub struct ObstacleIndex<'a> {
    course: &'a Course,
    solids: Vec<Solid>,
    solid_index: SpatialIndex,
    canopy_index: SpatialIndex,
}

#[derive(Clone, Copy, Debug)]
enum Solid {
    Trunk(usize),
    Wall(usize, usize),
    Rock(usize),
    Face([Vec3; 4]),
}

impl Course {
    /// Builds a reusable spatial index for obstacle, canopy and ray queries.
    pub fn obstacles(&self) -> ObstacleIndex<'_> {
        ObstacleIndex::new(self)
    }

    /// Returns ground height and the slopes along x and z, in that order.
    pub fn ground_at(&self, p: Vec2) -> Vec3 {
        let ground = self.ground_split_at(p);
        Vec3::new(ground.height(), ground.slope.x, ground.slope.y)
    }

    /// Builds an obstacle index for a one-off ray query.
    pub fn ray_hit(&self, origin: Vec3, direction: Vec3, max_m: Real) -> Option<Vec3> {
        self.obstacles().ray_hit(origin, direction, max_m)
    }
}

impl<'a> ObstacleIndex<'a> {
    pub fn new(course: &'a Course) -> Self {
        let data = course.data();
        let trees = &data.trees.instances;
        let mut solids = Vec::new();
        let mut bounds = Vec::new();
        let mut add = |solid, rect| {
            solids.push(solid);
            bounds.push(rect);
        };
        for (i, tree) in trees.iter().enumerate() {
            add(
                Solid::Trunk(i),
                point_bounds(tree.base).grow(tree.trunk_radius_m),
            );
        }
        for (i, wall) in data.walls.iter().enumerate() {
            for (k, pair) in wall.base.windows(2).enumerate() {
                add(
                    Solid::Wall(i, k),
                    Bounds::between(plan(pair[0]), plan(pair[1])).grow(wall.width_m * 0.5),
                );
            }
        }
        for (i, rock) in data.rocks.iter().enumerate() {
            add(
                Solid::Rock(i),
                point_bounds(rock.center).grow(rock.radii.x.max(rock.radii.z)),
            );
        }
        for face in &data.bunker_faces {
            for (base, top) in face.base.windows(2).zip(face.top.windows(2)) {
                let quad = [base[0], base[1], top[1], top[0]];
                add(Solid::Face(quad), Bounds::of(&quad.map(plan)));
            }
        }
        Self {
            course,
            solids,
            solid_index: SpatialIndex::new(bounds.into_iter()),
            canopy_index: SpatialIndex::new(
                trees
                    .iter()
                    .map(|tree| point_bounds(tree.base).grow(tree.crown_radius_m)),
            ),
        }
    }

    /// Finds the first entry into an inflated solid along the segment.
    pub fn obstacle_hit(&self, from: Vec3, to: Vec3, radius_m: Real) -> Option<ObstacleHit> {
        if !from.is_finite() || !to.is_finite() || !radius_m.is_finite() || radius_m < 0.0 {
            return None;
        }
        let data = self.course.data();
        // Along and across inflation can both contribute to the world-space extent of a wall.
        let search = Bounds::between(plan(from), plan(to)).grow(2.0 * radius_m);
        let mut best: Option<(usize, ObstacleHit)> = None;
        self.solid_index.visit(search, |i| {
            let hit = match self.solids[i] {
                Solid::Trunk(i) => hit_trunk(&data.trees.instances[i], from, to, radius_m),
                Solid::Wall(i, k) => hit_wall(&data.walls[i], k, from, to, radius_m),
                Solid::Rock(i) => hit_rock(&data.rocks[i], from, to, radius_m),
                Solid::Face(quad) => hit_face(quad, from, to, radius_m),
            };
            if let Some(hit) = hit
                && best.is_none_or(|(j, previous)| (hit.fraction, i) < (previous.fraction, j))
            {
                best = Some((i, hit));
            }
        });
        best.map(|(_, hit)| hit)
    }

    /// Returns the densest crown at this point, or zero in open air.
    pub fn canopy_density(&self, position: Vec3) -> Real {
        let mut density: Real = 0.0;
        self.canopy_index.visit(point_bounds(position), |i| {
            let tree = &self.course.data().trees.instances[i];
            let y0 = tree.base.y + tree.crown_base_m;
            let y1 = tree.base.y + tree.height_m;
            if position.y < y0 || position.y > y1 || y1 <= y0 || tree.crown_radius_m <= 0.0 {
                return;
            }
            let horizontal = (plan(position) - plan(tree.base)).length_squared();
            let radius = tree.crown_radius_m;
            let inside = if tree.cone {
                let width = radius * (y1 - position.y) / (y1 - y0);
                horizontal <= width * width
            } else {
                let mid = (y0 + y1) * 0.5;
                let vertical = (position.y - mid) / ((y1 - y0) * 0.5);
                horizontal / (radius * radius) + vertical * vertical <= 1.0
            };
            if inside {
                density = density.max(tree.density);
            }
        });
        density
    }

    pub fn ray_hit(&self, origin: Vec3, direction: Vec3, max_m: Real) -> Option<Vec3> {
        if !origin.is_finite() || !direction.is_finite() || !max_m.is_finite() || max_m <= 0.0 {
            return None;
        }
        let direction = direction.normalized();
        let above = |distance| {
            let p = origin + direction * distance;
            p.y - self.course.height_at(p)
        };
        let mut clearance = above(0.0);
        if clearance < 0.0 || direction == Vec3::ZERO {
            return None;
        }

        let end = origin + direction * max_m;
        let hit = self.obstacle_hit(origin, end, 0.0);
        let solid = hit.map_or(max_m, |hit| max_m * hit.fraction);
        let mut crown = None;
        let bounds = Bounds::between(plan(origin), plan(origin + direction * solid));
        self.canopy_index.visit(bounds, |i| {
            let tree = &self.course.data().trees.instances[i];
            if let Some(entry) = ray_crown_entry(tree, origin, direction, solid) {
                crown = Some(crown.map_or(entry, |old: Real| old.min(entry)));
            }
        });
        let limit = crown.unwrap_or(solid);
        let grade = self.course.maximum_grade() * plan(direction).length();
        let approach = (-direction.y + grade).max(0.05);
        let variation = direction.y.abs() + grade;
        let mut distance = 0.0;
        while distance < limit {
            let step = (clearance / approach).clamp(0.25, 25.0);
            let next = (distance + step).min(limit);
            let at_end = above(next);
            if let Some(entry) =
                first_ground_entry(&above, [distance, next], [clearance, at_end], variation)
            {
                return Some(origin + direction * entry);
            }
            distance = next;
            clearance = at_end;
        }
        if let Some(entry) = crown {
            Some(origin + direction * entry)
        } else {
            hit.map(|hit| hit.point)
        }
    }
}

fn first_ground_entry(
    above: &impl Fn(Real) -> Real,
    [lo, hi]: [Real; 2],
    [start, end]: [Real; 2],
    grade: Real,
) -> Option<Real> {
    if start.min(end) > grade * (hi - lo) * 0.5 {
        return None;
    }
    if hi - lo < RAY_RESOLUTION_M {
        return (end < 0.0).then_some(hi);
    }
    let mid = lo + (hi - lo) * 0.5;
    let clearance = above(mid);
    first_ground_entry(above, [lo, mid], [start, clearance], grade)
        .or_else(|| first_ground_entry(above, [mid, hi], [clearance, end], grade))
}

fn ray_crown_entry(tree: &Tree, origin: Vec3, direction: Vec3, limit: Real) -> Option<Real> {
    let height = tree.height_m - tree.crown_base_m;
    let radius = tree.crown_radius_m;
    if tree.density <= 0.0 || height <= 0.0 || radius <= 0.0 {
        return None;
    }
    let base = tree.base + UP * tree.crown_base_m;
    let start = origin - base;
    let (a, b, c) = if tree.cone {
        let slope = radius / height;
        let radial = plan(start);
        let travel = plan(direction);
        let from_tip = (height - start.y) * slope;
        let vertical = direction.y * slope;
        let radial_squared = radial.length_squared();
        let tip_squared = from_tip * from_tip;
        let base_contact = -start.y / direction.y;
        if start.y >= 0.0 && start.y <= height && radial_squared <= tip_squared {
            return Some(0.0);
        }
        let base_entry = if (0.0..=limit).contains(&base_contact)
            && direction.y > 0.0
            && plan(start + direction * base_contact).length_squared() <= radius * radius
        {
            base_contact
        } else {
            Real::INFINITY
        };
        let travel_squared = travel.length_squared();
        let vertical_squared = vertical * vertical;
        let mixed = from_tip * vertical;
        let a = travel_squared - vertical_squared;
        let b = radial.dot(travel) + mixed;
        let c = radial_squared - tip_squared;
        let b_scale = (radial.x * travel.x).abs() + (radial.y * travel.y).abs() + mixed.abs();
        // Normalization and coefficient arithmetic can leave roundoff after cancellation.
        // Scale each residual by the magnitudes of its terms before they cancel.
        let roundoff = 8.0 * Real::EPSILON;
        if direction.y != 0.0
            && a.abs() <= roundoff * (travel_squared + vertical_squared)
            && b.abs() <= roundoff * b_scale
            && c.abs() <= roundoff * (radial_squared + tip_squared)
        {
            let tip_contact = (height - start.y) / direction.y;
            let entry = base_contact.min(tip_contact).max(0.0);
            let exit = base_contact.max(tip_contact).min(limit);
            return (entry <= exit).then_some(entry);
        }
        return [base_entry]
            .into_iter()
            .chain(ray_roots(a, b, c))
            .filter(|&s| {
                (0.0..=limit).contains(&s)
                    && (-1e-12..=height + 1e-12).contains(&(start.y + direction.y * s))
            })
            .min_by(Real::total_cmp);
    } else {
        let radii = Vec3::new(radius, 0.5 * height, radius);
        let p = (start - UP * (0.5 * height)).cdiv(radii);
        let v = direction.cdiv(radii);
        (v.length_squared(), p.dot(v), p.length_squared() - 1.0)
    };
    if c <= 0.0 {
        return Some(0.0);
    }
    ray_roots(a, b, c)
        .into_iter()
        .filter(|&s| (0.0..=limit).contains(&s))
        .min_by(Real::total_cmp)
}

fn ray_roots(a: Real, b: Real, c: Real) -> [Real; 2] {
    if a == 0.0 {
        if b != 0.0 {
            return [-c / (2.0 * b), Real::INFINITY];
        }
        return [Real::INFINITY; 2];
    }
    let determinant = b * b - a * c;
    let nearer = -b - fm::sqrt(determinant).copysign(b);
    if nearer == 0.0 {
        [0.0, 0.0]
    } else {
        [nearer / a, c / nearer]
    }
}

fn point_bounds(p: Vec3) -> Bounds {
    let p = plan(p);
    Bounds::between(p, p)
}

fn entry_fraction(a: Real, b: Real, c: Real) -> Option<Real> {
    let discriminant = b * b - 4.0 * a * c;
    if a < 1e-12 || c < 0.0 || discriminant < 0.0 {
        return None;
    }
    let fraction = (-b - fm::sqrt(discriminant)) / (2.0 * a);
    (0.0..=1.0).contains(&fraction).then_some(fraction)
}

fn hit_trunk(tree: &Tree, from: Vec3, to: Vec3, radius: Real) -> Option<ObstacleHit> {
    // A trunk uses the side intersection, with its height checked at that point.
    let a = plan(from) - plan(tree.base);
    let d = plan(to) - plan(from);
    let r = tree.trunk_radius_m + radius;
    let fraction = entry_fraction(d.dot(d), 2.0 * a.dot(d), a.dot(a) - r * r)?;
    let point = from.lerp(to, fraction);
    let top = if tree.cone {
        tree.height_m
    } else {
        tree.crown_base_m + 0.5 * (tree.height_m - tree.crown_base_m)
    };
    if point.y < tree.base.y - radius || point.y > tree.base.y + top {
        return None;
    }
    Some(ObstacleHit {
        kind: ObstacleKind::Trunk,
        point,
        normal: world((plan(point) - plan(tree.base)).normalized(), 0.0),
        fraction,
    })
}

fn hit_wall(wall: &Wall, k: usize, from: Vec3, to: Vec3, radius: Real) -> Option<ObstacleHit> {
    let (a, b) = (wall.base[k], wall.base[k + 1]);
    let along = plan(b) - plan(a);
    let length = along.length();
    if length < 1e-6 {
        return None;
    }
    let u = along / length;
    let v = Vec2::new(-u.y, u.x);
    let o = plan(from) - plan(a);
    let d = plan(to) - plan(from);
    let half = wall.width_m * 0.5 + radius;
    let lo = [-radius, -half, a.y.min(b.y) - 2.0];
    let hi = [length + radius, half, a.y.max(b.y) + wall.height_m + radius];
    let start = [o.dot(u), o.dot(v), from.y];
    let delta = [d.dot(u), d.dot(v), to.y - from.y];
    let axes = [world(u, 0.0), world(v, 0.0), UP];
    let mut entry = 0.0;
    let mut exit: Real = 1.0;
    let mut normal = None;
    for i in 0..3 {
        if delta[i].abs() < 1e-12 {
            if !(lo[i]..=hi[i]).contains(&start[i]) {
                return None;
            }
            continue;
        }
        let a = (lo[i] - start[i]) / delta[i];
        let b = (hi[i] - start[i]) / delta[i];
        if a.min(b) > entry {
            entry = a.min(b);
            normal = Some(if a <= b { -axes[i] } else { axes[i] });
        }
        exit = exit.min(a.max(b));
        if entry > exit {
            return None;
        }
    }
    Some(ObstacleHit {
        kind: ObstacleKind::Wall,
        point: from.lerp(to, entry),
        normal: normal?,
        fraction: entry,
    })
}

fn hit_rock(rock: &Rock, from: Vec3, to: Vec3, radius: Real) -> Option<ObstacleHit> {
    let radii = rock.radii + Vec3::new(radius, radius, radius);
    let basis = Basis::from_axis_angle(UP, rock.yaw_rad);
    let inverse = basis.inverse();
    let a = inverse.xform(from - rock.center).cdiv(radii);
    let b = inverse.xform(to - rock.center).cdiv(radii);
    let d = b - a;
    let fraction = entry_fraction(d.dot(d), 2.0 * a.dot(d), a.dot(a) - 1.0)?;
    let local = a + d * fraction;
    Some(ObstacleHit {
        kind: ObstacleKind::Rock,
        point: from.lerp(to, fraction),
        normal: basis.xform(local.cdiv(radii)).normalized(),
        fraction,
    })
}

fn hit_face([a, b, c, d]: [Vec3; 4], from: Vec3, to: Vec3, radius: Real) -> Option<ObstacleHit> {
    [[a, b, c], [a, c, d]]
        .into_iter()
        .filter_map(|[a, b, c]| {
            let mut normal = (b - a).cross(c - a).normalized();
            if normal.dot(from - a) < 0.0 {
                normal = -normal;
            }
            let shift = normal * radius;
            let fraction = triangle_fraction(from, to, a + shift, b + shift, c + shift)?;
            Some(ObstacleHit {
                kind: ObstacleKind::BunkerFace,
                point: from.lerp(to, fraction),
                normal,
                fraction,
            })
        })
        .min_by(|a, b| a.fraction.total_cmp(&b.fraction))
}

fn triangle_fraction(from: Vec3, to: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<Real> {
    let direction = to - from;
    let e1 = b - a;
    let e2 = c - a;
    let p = direction.cross(e2);
    let determinant = e1.dot(p);
    if determinant.abs() < 1e-12 {
        return None;
    }
    let inverse = 1.0 / determinant;
    let offset = from - a;
    let u = offset.dot(p) * inverse;
    let q = offset.cross(e1);
    let v = direction.dot(q) * inverse;
    if u < 0.0 || v < 0.0 || u + v > 1.0 {
        return None;
    }
    let fraction = e2.dot(q) * inverse;
    (0.0..=1.0).contains(&fraction).then_some(fraction)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ray_ground_entry_can_precede_an_above_ground_step_end() {
        let clearance = |d: Real| (d - 2.0) * (d - 3.0);
        let entry = first_ground_entry(&clearance, [0.0, 4.0], [6.0, 2.0], 5.0).unwrap();
        assert!((2.0..2.0 + RAY_RESOLUTION_M).contains(&entry));
    }

    #[test]
    fn crown_between_march_samples_has_a_first_entry() {
        let tree = Tree {
            id: "synthetic".into(),
            species_index: 0,
            base: Vec3::new(2.0, 0.0, 0.0),
            height_m: 5.0,
            trunk_radius_m: 0.05,
            crown_base_m: 1.0,
            crown_radius_m: 0.25,
            density: 1.0,
            cone: false,
        };
        let origin = Vec3::new(0.0, 3.0, 0.0);
        let along = Vec3::new(1.0, 0.0, 0.0);
        assert_eq!(ray_crown_entry(&tree, origin, along, 4.0), Some(1.75));
        assert_eq!(
            ray_crown_entry(&tree, origin + along * 2.0, along, 1.0),
            Some(0.0)
        );
    }

    #[test]
    fn ray_along_a_conical_crown_hits_first_at_the_tip() {
        let tree = Tree {
            id: "synthetic".into(),
            species_index: 0,
            base: Vec3::ZERO,
            height_m: 5.0,
            trunk_radius_m: 0.05,
            crown_base_m: 1.0,
            crown_radius_m: 4.0,
            density: 1.0,
            cone: true,
        };
        let origin = Vec3::new(1.0, 6.0, 0.0);
        let direction = Vec3::new(-1.0, -1.0, 0.0).normalized();
        let distance = ray_crown_entry(&tree, origin, direction, 6.0).unwrap();
        assert!((distance - fm::sqrt(2.0)).abs() <= 1e-12);
        assert_eq!(ray_crown_entry(&tree, origin, direction, 1.0), None);
        assert_eq!(ray_crown_entry(&tree, origin, -direction, 6.0), None);
    }

    #[test]
    fn normalized_cone_side_ray_hits_the_tip() {
        let tree = Tree {
            id: "synthetic".into(),
            species_index: 0,
            base: Vec3::ZERO,
            height_m: 5.0,
            trunk_radius_m: 0.05,
            crown_base_m: 1.0,
            crown_radius_m: 5.0,
            density: 1.0,
            cone: true,
        };
        let origin = Vec3::new(3.0, 9.0, 4.0);
        let direction = Vec3::new(-3.0, -4.0, -4.0).normalized();
        let distance = ray_crown_entry(&tree, origin, direction, 7.0).expect("ray hits the tip");
        assert!((distance - fm::sqrt(41.0)).abs() <= 1e-12);
        assert_eq!(ray_crown_entry(&tree, origin, direction, 6.0), None);
        let below = Vec3::new(-6.0, -3.0, -8.0);
        let distance = ray_crown_entry(&tree, below, -direction, 7.0).expect("ray hits the base");
        assert!((distance - fm::sqrt(41.0)).abs() <= 1e-12);
    }
}
