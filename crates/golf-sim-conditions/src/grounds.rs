//! Ground samplers for a course with uniform turf conditions and for practice.

use std::sync::Arc;

use golf_sim_base::{Real, Surface, Vec2, Vec3};
use golf_sim_course::{course::Course, legal::LegalMap, obstacles::ObstacleIndex, zones::ZoneMap};
use golf_sim_physics::{
    plane::PlaneGround,
    samplers::{GroundSample, GroundSampler, ZoneCrossing, is_long_grass},
    values::{ObstacleHit, Zone},
};

#[derive(Debug)]
pub struct CourseGround<'a> {
    course: &'a Course,
    obstacles: Arc<ObstacleIndex<'a>>,
    zones: Arc<ZoneMap<'a>>,
    legal: Arc<LegalMap<'a>>,
    firmness: Real,
    moisture: Real,
    green_speed_m: Real,
}

impl<'a> CourseGround<'a> {
    pub fn new(course: &'a Course, firmness: Real, moisture: Real, green_speed_m: Real) -> Self {
        Self {
            course,
            obstacles: Arc::new(course.obstacles()),
            zones: Arc::new(course.zone_map()),
            legal: Arc::new(course.legal()),
            firmness,
            moisture,
            green_speed_m,
        }
    }

    pub fn with_turf(&self, firmness: Real, moisture: Real, green_speed_m: Real) -> Self {
        Self {
            course: self.course,
            obstacles: Arc::clone(&self.obstacles),
            zones: Arc::clone(&self.zones),
            legal: Arc::clone(&self.legal),
            firmness,
            moisture,
            green_speed_m,
        }
    }

    pub fn legal(&self) -> &LegalMap<'a> {
        &self.legal
    }
}

impl GroundSampler for CourseGround<'_> {
    fn sample_ground(&self, x: Real, z: Real) -> GroundSample {
        let point = Vec2::new(x, z);
        let height = self.course.ground_split_at(point);
        let mut sample = GroundSample {
            surface: Surface::Rough,
            height_m: height.height(),
            normal: Vec3::new(-height.slope.x, 1.0, -height.slope.y).normalized(),
            firmness: self.firmness,
            moisture: self.moisture,
            ..GroundSample::default()
        };
        if let Some((_, grid)) = self.course.surface_at(point) {
            let material = &self.course.data().materials[usize::from(grid.material_at(point).0)];
            sample.surface = material.kind;
            if is_long_grass(sample.surface) {
                sample.grass_density = material.grass_density;
                if material.grass_height_m >= 0.0 {
                    sample.grass_height_m = material.grass_height_m * grid.stand_at(point);
                }
            }
            if matches!(sample.surface, Surface::Green | Surface::Fringe) {
                sample.green_speed_m = self.green_speed_m;
            }
        }
        sample
    }

    fn sample_strike_ground(&self, x: Real, z: Real, lie_surface: Surface) -> GroundSample {
        let mut sample = self.sample_ground(x, z);
        // A short-grass lie can still meet long grass near a mowing edge.
        if !is_long_grass(lie_surface) {
            sample.grass_density = -1.0;
        }
        sample
    }

    fn obstacle_hit(&self, from: Vec3, to: Vec3, radius_m: Real) -> Option<ObstacleHit> {
        self.obstacles.obstacle_hit(from, to, radius_m)
    }

    fn canopy_density(&self, position: Vec3) -> Real {
        self.obstacles.canopy_density(position)
    }

    fn has_zones(&self) -> bool {
        true
    }

    fn zone_at(&self, position: Vec3) -> Zone {
        self.zones.zone_at(position)
    }

    fn crossings(&self, from: Vec3, to: Vec3, from_zone: Zone, out: &mut Vec<ZoneCrossing>) {
        self.zones.crossings(from, to, from_zone, out);
    }

    fn maximum_grade(&self) -> Real {
        self.course.maximum_grade()
    }
}

#[derive(Clone, Debug)]
pub struct PracticeGround {
    plane: PlaneGround,
    pad: Option<Pad>,
}

#[derive(Clone, Copy, Debug)]
struct Pad {
    centre: Vec2,
    gradient: Vec2,
    flat_radius_m: Real,
    blend_radius_m: Real,
}

impl PracticeGround {
    pub fn new(plane: PlaneGround) -> Self {
        Self { plane, pad: None }
    }

    /// Adds a pad rising at `grade` in the horizontal direction of `uphill`.
    pub fn tilt_pad(
        &mut self,
        centre: Vec3,
        uphill: Vec3,
        grade: Real,
        flat_radius_m: Real,
        blend_radius_m: Real,
    ) -> &mut Self {
        let gradient = Vec2::new(uphill.x, uphill.z).normalized() * grade;
        self.pad = (gradient != Vec2::ZERO).then_some(Pad {
            centre: Vec2::new(centre.x, centre.z),
            gradient,
            flat_radius_m,
            blend_radius_m,
        });
        self
    }

    pub fn has_pad(&self) -> bool {
        self.pad.is_some()
    }
}

impl GroundSampler for PracticeGround {
    fn sample_ground(&self, x: Real, z: Real) -> GroundSample {
        let mut sample = self.plane.sample_ground(x, z);
        let Some(pad) = self.pad else {
            return sample;
        };
        let offset = Vec2::new(x, z) - pad.centre;
        let distance = offset.length();
        if distance >= pad.blend_radius_m {
            return sample;
        }

        let span = pad.blend_radius_m - pad.flat_radius_m;
        let t = ((distance - pad.flat_radius_m) / span).clamp(0.0, 1.0);
        let weight = 1.0 - t * t * (3.0 - 2.0 * t);
        let rise = pad.gradient.dot(offset);
        let mut slope = Vec2::new(-sample.normal.x, -sample.normal.z) / sample.normal.y;
        let pad_slope = pad.gradient * weight;
        slope.x += pad_slope.x;
        slope.y += pad_slope.y;
        // The normal needs the derivative of the blend as well as the pad's weighted slope.
        if distance > pad.flat_radius_m {
            let weight_slope = -6.0 * t * (1.0 - t) / span;
            let blend_slope = offset / distance * (rise * weight_slope);
            slope.x += blend_slope.x;
            slope.y += blend_slope.y;
        }
        sample.height_m += rise * weight;
        sample.normal = Vec3::new(-slope.x, 1.0, -slope.y).normalized();
        sample
    }

    fn obstacle_hit(&self, from: Vec3, to: Vec3, radius_m: Real) -> Option<ObstacleHit> {
        self.plane.obstacle_hit(from, to, radius_m)
    }

    fn canopy_density(&self, position: Vec3) -> Real {
        self.plane.canopy_density(position)
    }

    fn has_zones(&self) -> bool {
        self.plane.has_zones()
    }

    fn zone_at(&self, position: Vec3) -> Zone {
        self.plane.zone_at(position)
    }

    fn crossings(&self, from: Vec3, to: Vec3, from_zone: Zone, out: &mut Vec<ZoneCrossing>) {
        self.plane.crossings(from, to, from_zone, out);
    }

    fn is_planar(&self) -> bool {
        !self.has_pad() && self.plane.is_planar()
    }

    fn is_uniform_flat(&self) -> bool {
        !self.has_pad() && self.plane.is_uniform_flat()
    }

    fn maximum_grade(&self) -> Real {
        let Some(pad) = self.pad else {
            return self.plane.maximum_grade();
        };
        let blend = pad.blend_radius_m - pad.flat_radius_m;
        let base = self.plane.sample_ground(pad.centre.x, pad.centre.y).normal;
        Vec2::new(base.x, base.z).length() / base.y
            + pad.gradient.length() * (1.0 + 1.5 * pad.blend_radius_m / blend)
    }
}

/// The native grounds a constant-day world can expose to physics.
#[derive(Debug)]
pub enum Ground<'a> {
    Course(Box<CourseGround<'a>>),
    Practice(PracticeGround),
    Plane(PlaneGround),
}

impl<'a> From<PracticeGround> for Ground<'a> {
    fn from(ground: PracticeGround) -> Self {
        Self::Practice(ground)
    }
}

impl<'a> From<PlaneGround> for Ground<'a> {
    fn from(ground: PlaneGround) -> Self {
        Self::Plane(ground)
    }
}

impl GroundSampler for Ground<'_> {
    fn sample_ground(&self, x: Real, z: Real) -> GroundSample {
        match self {
            Self::Course(ground) => ground.sample_ground(x, z),
            Self::Practice(ground) => ground.sample_ground(x, z),
            Self::Plane(ground) => ground.sample_ground(x, z),
        }
    }

    fn sample_strike_ground(&self, x: Real, z: Real, lie_surface: Surface) -> GroundSample {
        match self {
            Self::Course(ground) => ground.sample_strike_ground(x, z, lie_surface),
            _ => self.sample_ground(x, z),
        }
    }

    fn obstacle_hit(&self, from: Vec3, to: Vec3, radius_m: Real) -> Option<ObstacleHit> {
        match self {
            Self::Course(ground) => ground.obstacle_hit(from, to, radius_m),
            Self::Practice(ground) => ground.obstacle_hit(from, to, radius_m),
            Self::Plane(ground) => ground.obstacle_hit(from, to, radius_m),
        }
    }

    fn canopy_density(&self, position: Vec3) -> Real {
        match self {
            Self::Course(ground) => ground.canopy_density(position),
            Self::Practice(ground) => ground.canopy_density(position),
            Self::Plane(ground) => ground.canopy_density(position),
        }
    }

    fn has_zones(&self) -> bool {
        match self {
            Self::Course(ground) => ground.has_zones(),
            Self::Practice(ground) => ground.has_zones(),
            Self::Plane(ground) => ground.has_zones(),
        }
    }

    fn zone_at(&self, position: Vec3) -> Zone {
        match self {
            Self::Course(ground) => ground.zone_at(position),
            Self::Practice(ground) => ground.zone_at(position),
            Self::Plane(ground) => ground.zone_at(position),
        }
    }

    fn crossings(&self, from: Vec3, to: Vec3, from_zone: Zone, out: &mut Vec<ZoneCrossing>) {
        match self {
            Self::Course(ground) => ground.crossings(from, to, from_zone, out),
            Self::Practice(ground) => ground.crossings(from, to, from_zone, out),
            Self::Plane(ground) => ground.crossings(from, to, from_zone, out),
        }
    }

    fn is_planar(&self) -> bool {
        match self {
            Self::Course(ground) => ground.is_planar(),
            Self::Practice(ground) => ground.is_planar(),
            Self::Plane(ground) => ground.is_planar(),
        }
    }

    fn is_uniform_flat(&self) -> bool {
        match self {
            Self::Course(ground) => ground.is_uniform_flat(),
            Self::Practice(ground) => ground.is_uniform_flat(),
            Self::Plane(ground) => ground.is_uniform_flat(),
        }
    }

    fn maximum_grade(&self) -> Real {
        match self {
            Self::Course(ground) => ground.maximum_grade(),
            Self::Practice(ground) => ground.maximum_grade(),
            Self::Plane(ground) => ground.maximum_grade(),
        }
    }
}
