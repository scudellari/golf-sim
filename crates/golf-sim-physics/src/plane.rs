//! A plane with optional surface patches, obstacles and rules zones, and constant air.

use golf_sim_base::{Real, Vec2, Vec3, num::clampf};

use crate::{
    samplers::{AirSample, AirSampler, GroundSample, GroundSampler, ZoneCrossing},
    tunables::{CONTACT_SLOP_M, SOLID_SWEEP_ITERATIONS},
    values::{ObstacleHit, ObstacleKind, UP, Zone, xz},
};

#[derive(Clone, Debug, Default)]
pub struct PlaneGround {
    base: GroundSample,
    origin: Vec3,
    patches: Vec<(Rect, GroundSample)>,
    trunks: Vec<Trunk>,
    canopies: Vec<Canopy>,
    zones: Vec<(Rect, Zone)>,
    outside_zone: Zone,
}

impl PlaneGround {
    pub fn new(mut base: GroundSample, origin: Vec3) -> Self {
        base.normal = base.normal.normalized();
        Self {
            base,
            origin,
            ..Self::default()
        }
    }

    pub fn sloped(mut base: GroundSample, uphill: Vec3, grade: Real) -> Self {
        let direction = Vec3::new(uphill.x, 0.0, uphill.z).normalized();
        base.normal = (UP - direction * grade).normalized();
        Self {
            base,
            ..Self::default()
        }
    }

    pub fn add_patch(&mut self, min: Vec2, max: Vec2, sample: GroundSample) -> &mut Self {
        self.patches.push((Rect { min, max }, sample));
        self
    }

    pub fn add_zone(&mut self, min: Vec2, max: Vec2, zone: Zone) -> &mut Self {
        self.zones.push((Rect { min, max }, zone));
        self
    }

    pub fn set_outside_zone(&mut self, zone: Zone) {
        self.outside_zone = zone;
    }

    pub fn add_trunk(&mut self, centre: Vec2, radius_m: Real, height_m: Real) -> &mut Self {
        self.trunks.push(Trunk {
            centre,
            radius_m,
            height_m,
        });
        self
    }

    pub fn add_canopy(&mut self, centre: Vec3, radius_m: Real, density: Real) -> &mut Self {
        self.canopies.push(Canopy {
            centre,
            radius_m,
            density,
        });
        self
    }

    pub fn height_at(&self, x: Real, z: Real) -> Real {
        let normal = self.base.normal;
        self.origin.y - (normal.x * (x - self.origin.x) + normal.z * (z - self.origin.z)) / normal.y
    }
}

impl GroundSampler for PlaneGround {
    fn sample_ground(&self, x: Real, z: Real) -> GroundSample {
        let point = Vec2::new(x, z);
        let source = self
            .patches
            .iter()
            .rev()
            .find(|(rectangle, _)| rectangle.contains(point))
            .map_or(self.base, |(_, sample)| *sample);
        GroundSample {
            height_m: self.height_at(x, z),
            normal: self.base.normal,
            ..source
        }
    }

    fn obstacle_hit(&self, from: Vec3, to: Vec3, radius_m: Real) -> Option<ObstacleHit> {
        let mut best = None;
        for trunk in &self.trunks {
            let Vec2 { x, y: z } = trunk.centre;
            let base = Vec3::new(x, self.height_at(x, z), z);
            if let Some(hit) = sweep_cylinder(
                from,
                to,
                base,
                trunk.height_m,
                trunk.radius_m,
                radius_m,
                ObstacleKind::Trunk,
            ) && best.is_none_or(|old: ObstacleHit| hit.fraction < old.fraction)
            {
                best = Some(hit);
            }
        }
        best
    }

    fn canopy_density(&self, position: Vec3) -> Real {
        self.canopies
            .iter()
            .filter(|canopy| position.distance_to(canopy.centre) <= canopy.radius_m)
            .fold(0.0, |density, canopy| density.max(canopy.density))
    }

    fn has_zones(&self) -> bool {
        !self.zones.is_empty() || self.outside_zone != Zone::InPlay
    }

    fn zone_at(&self, position: Vec3) -> Zone {
        let point = xz(position);
        self.zones
            .iter()
            .rev()
            .find(|(rectangle, _)| rectangle.contains(point))
            .map_or(self.outside_zone, |(_, zone)| *zone)
    }

    fn crossings(&self, from: Vec3, to: Vec3, from_zone: Zone, out: &mut Vec<ZoneCrossing>) {
        const MERGE_M: Real = 1e-6;

        out.clear();
        let start = xz(from);
        let delta = xz(to) - start;
        if delta == Vec2::ZERO || self.zones.is_empty() {
            return;
        }

        let mut hits = Vec::new();
        for (rectangle, _) in &self.zones {
            rectangle.collect_crossings(start, delta, &mut hits);
        }
        hits.sort_unstable_by(Real::total_cmp);

        let length = delta.length();
        let mut zone = from_zone;
        let mut i = 0;
        while let Some(&at) = hits.get(i) {
            i += 1;
            while hits
                .get(i)
                .is_some_and(|next| (next - at) * length < MERGE_M)
            {
                i += 1;
            }
            let until = hits.get(i).copied().unwrap_or(1.0);
            // A segment that ends on an edge owns the crossing, so sample just beyond its end.
            let probe = if (until - at) * length >= MERGE_M {
                (at + until) * 0.5
            } else {
                at + MERGE_M / length
            };
            let point = Vec2::new(start.x + delta.x * probe, start.y + delta.y * probe);
            let entered = self.zone_at(Vec3::new(point.x, 0.0, point.y));
            if entered != zone {
                out.push(ZoneCrossing {
                    fraction: at,
                    zone: entered,
                });
                zone = entered;
            }
        }
    }

    fn is_planar(&self) -> bool {
        true
    }

    fn is_uniform_flat(&self) -> bool {
        self.patches.is_empty() && self.base.normal == UP
    }
}

#[derive(Clone, Copy, Debug)]
struct Rect {
    min: Vec2,
    max: Vec2,
}

impl Rect {
    fn contains(self, point: Vec2) -> bool {
        (self.min.x..=self.max.x).contains(&point.x) && (self.min.y..=self.max.y).contains(&point.y)
    }

    fn collect_crossings(self, start: Vec2, delta: Vec2, hits: &mut Vec<Real>) {
        const CORNER_SLACK: Real = 1e-9;
        let corners = [
            self.min,
            Vec2::new(self.max.x, self.min.y),
            self.max,
            Vec2::new(self.min.x, self.max.y),
        ];
        for (&from, &to) in corners.iter().zip(corners.iter().cycle().skip(1)) {
            let edge = to - from;
            let denominator = delta.cross(edge);
            if denominator == 0.0 {
                continue;
            }
            let offset = from - start;
            let along_path = offset.cross(edge) / denominator;
            let along_edge = offset.cross(delta) / denominator;
            if along_path > 0.0
                && along_path <= 1.0
                && (-CORNER_SLACK..=1.0 + CORNER_SLACK).contains(&along_edge)
            {
                hits.push(along_path);
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Trunk {
    centre: Vec2,
    radius_m: Real,
    height_m: Real,
}

#[derive(Clone, Copy, Debug)]
struct Canopy {
    centre: Vec3,
    radius_m: Real,
    density: Real,
}

pub(crate) fn sweep_cylinder(
    from: Vec3,
    to: Vec3,
    base: Vec3,
    height: Real,
    radius: Real,
    ball_radius: Real,
    kind: ObstacleKind,
) -> Option<ObstacleHit> {
    let a = from - base;
    let b = to - base;
    let reach = radius + ball_radius;
    if a.x.min(b.x) > reach
        || a.x.max(b.x) < -reach
        || a.z.min(b.z) > reach
        || a.z.max(b.z) < -reach
        || a.y.min(b.y) > height + ball_radius
        || a.y.max(b.y) < -ball_radius
    {
        return None;
    }
    let delta = b - a;
    let length = delta.length();
    if length == 0.0 {
        return None;
    }

    let mut fraction = 0.0;
    for _ in 0..SOLID_SWEEP_ITERATIONS {
        let point = a.lerp(b, fraction);
        let horizontal = xz(point);
        let radial = horizontal.limit_length(radius);
        let mut nearest = Vec3::new(radial.x, clampf(point.y, 0.0, height), radial.y);
        let offset = point - nearest;
        let distance = offset.length();
        if distance <= ball_radius + CONTACT_SLOP_M * 0.1 {
            let mut normal = offset.normalized();
            if normal == Vec3::ZERO {
                // A centre already inside the cylinder needs the nearest escape face.
                let side_distance = radius - horizontal.length();
                if point.y.min(height - point.y) <= side_distance {
                    if point.y < height * 0.5 {
                        normal = -UP;
                        nearest.y = 0.0;
                    } else {
                        normal = UP;
                        nearest.y = height;
                    }
                } else {
                    let side = if horizontal == Vec2::ZERO {
                        Vec2::new(1.0, 0.0)
                    } else {
                        horizontal.normalized()
                    };
                    normal = Vec3::new(side.x, 0.0, side.y);
                    nearest = point + normal * side_distance;
                }
            }
            return (delta.dot(normal) < 0.0).then(|| ObstacleHit {
                kind,
                point: base + nearest + normal * ball_radius,
                normal,
                fraction,
            });
        }
        fraction += (distance - ball_radius) / length;
        if fraction > 1.0 {
            break;
        }
    }
    None
}

#[derive(Clone, Debug, Default)]
pub struct ConstantAir {
    pub air: AirSample,
    gusts: Vec<Gust>,
}

impl ConstantAir {
    pub fn new(air: AirSample) -> Self {
        Self {
            air,
            gusts: Vec::new(),
        }
    }

    pub fn add_gust(&mut self, start_s: Real, end_s: Real, gust_mps: Vec3) -> &mut Self {
        self.gusts.push(Gust {
            start_s,
            end_s,
            gust_mps,
        });
        self
    }
}

impl AirSampler for ConstantAir {
    fn sample_air(&self, time_s: Real, _position: Vec3) -> AirSample {
        let mut air = self.air;
        for gust in &self.gusts {
            if (gust.start_s..gust.end_s).contains(&time_s) {
                air.gust_mps += gust.gust_mps;
            }
        }
        air
    }
}

#[derive(Clone, Copy, Debug)]
struct Gust {
    start_s: Real,
    end_s: Real,
    gust_mps: Vec3,
}
