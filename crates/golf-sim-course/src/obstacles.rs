use golf_sim_base::{Basis, Real, Vec2, Vec3, fm};
use golf_sim_physics::values::{ObstacleHit, ObstacleKind};

use crate::{
    course::{Course, Rock, Tree, Wall},
    zones::{Bounds, SpatialIndex, plan, world},
};

const UP: Vec3 = Vec3::new(0.0, 1.0, 0.0);

const RAY_REFINE_STEPS: usize = 14;

#[derive(Debug)]
pub struct ObstacleIndex<'a> {
    course: &'a Course,
    solids: Vec<Solid>,
    solid_index: SpatialIndex,
    canopy_index: SpatialIndex,
    tallest_tree_m: Real,
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
            tallest_tree_m: trees.iter().map(|tree| tree.height_m).fold(0.0, Real::max),
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
        if !origin.is_finite() || !direction.is_finite() || !max_m.is_finite() {
            return None;
        }
        let d = direction.normalized();
        let mut at = origin;
        let mut above = at.y - self.course.height_at(at);
        if above < 0.0 || d == Vec3::ZERO {
            return None;
        }
        let grade = self.course.maximum_grade();
        let approach = (-d.y + grade * plan(d).length()).max(0.05);
        let mut travelled = 0.0;
        while travelled < max_m {
            let mut step = (above / approach).clamp(0.25, 25.0);
            if above < self.tallest_tree_m + 1.0 {
                step = step.min(1.0);
            }
            let next = at + d * step;
            let ground = next.y - self.course.height_at(next);
            let hit = self.obstacle_hit(at, next, 0.0);
            if ground < 0.0 {
                let fraction = self.ground_fraction(at, next);
                return Some(match hit {
                    Some(hit) if hit.fraction <= fraction => hit.point,
                    _ => at.lerp(next, fraction),
                });
            }
            if let Some(hit) = hit {
                return Some(hit.point);
            }
            if self.canopy_density(next) > 0.0 {
                let (mut outside, mut inside) = (at, next);
                for _ in 0..RAY_REFINE_STEPS {
                    let mid = (outside + inside) * 0.5;
                    if self.canopy_density(mid) > 0.0 {
                        inside = mid;
                    } else {
                        outside = mid;
                    }
                }
                return Some(inside);
            }
            at = next;
            above = ground;
            travelled += step;
        }
        None
    }

    fn ground_fraction(&self, from: Vec3, to: Vec3) -> Real {
        let mut lo = 0.0;
        let mut hi = 1.0;
        for _ in 0..RAY_REFINE_STEPS {
            let mid = (lo + hi) * 0.5;
            let p = from.lerp(to, mid);
            if p.y >= self.course.height_at(p) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        hi
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
