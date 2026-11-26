use std::{collections::BTreeMap, path::Path, sync::Arc};

use golf_sim_base::{Error, Material, Real, Surface, Vec2, Vec3, variant::Variant};

use crate::{
    bundle,
    grid::{HeightGrid, SplitHeight, SurfaceGrid},
};

const FINE_BLEND_M: Real = 1.0;
const PROPERTY_BLEND_M: Real = 4.0;

#[derive(Debug, PartialEq)]
pub struct Course {
    revision: String,
    data: CourseData,
}

impl Course {
    pub fn open(dir: &Path) -> Result<Arc<Self>, Error> {
        bundle::open(dir)
    }

    /// Parse a decompressed course payload and compute its revision from the original bytes.
    pub fn parse(bytes: &[u8]) -> Result<Arc<Self>, Error> {
        bundle::parse(bytes)
    }

    pub(crate) fn new(revision: String, data: CourseData) -> Arc<Self> {
        Arc::new(Self { revision, data })
    }

    pub fn revision(&self) -> &str {
        &self.revision
    }

    pub fn data(&self) -> &CourseData {
        &self.data
    }

    pub fn hole(&self, number: i64) -> Option<&Hole> {
        self.data.holes.iter().find(|hole| hole.number == number)
    }

    pub fn maximum_grade(&self) -> Real {
        self.data.grade_bound
    }

    pub fn height_at(&self, position: Vec3) -> Real {
        self.ground_split_at(Vec2::new(position.x, position.z))
            .height()
    }

    pub fn normal_at(&self, position: Vec3) -> Vec3 {
        let slope = self
            .ground_split_at(Vec2::new(position.x, position.z))
            .slope;
        Vec3::new(-slope.x, 1.0, -slope.y).normalized()
    }

    pub fn material_at(&self, position: Vec3) -> Option<&GroundMaterial> {
        let index = self.material_index_at(Vec2::new(position.x, position.z))?;
        Some(&self.data.materials[usize::from(index.0)])
    }

    pub fn material_index_at(&self, p: Vec2) -> Option<Material> {
        self.surface_at(p).map(|(_, grid)| grid.material_at(p))
    }

    pub fn surface_at(&self, p: Vec2) -> Option<(SurfaceId, &SurfaceGrid)> {
        for (index, tile) in self.data.surface_tiles.iter().enumerate() {
            if tile.frame().contains(p) {
                return Some((SurfaceId::Tile(index), tile));
            }
        }
        let property = &self.data.property.as_ref()?.surface;
        property
            .frame()
            .contains(p)
            .then_some((SurfaceId::Property, property))
    }

    pub fn ground_split_at(&self, p: Vec2) -> SplitHeight {
        for tile in &self.data.fine_tiles {
            let depth = tile.inner_distance(p);
            if depth >= FINE_BLEND_M {
                return tile.sample_split(p);
            }
            if depth > 0.0 {
                return self.coarse_split_at(p).blend(
                    tile.sample_split(p),
                    depth,
                    FINE_BLEND_M,
                    tile.inner_direction(p),
                );
            }
        }
        self.coarse_split_at(p)
    }

    fn coarse_split_at(&self, p: Vec2) -> SplitHeight {
        let mut tile = &self.data.terrain_tiles[0];
        let mut depth = tile.inner_distance(p);
        for candidate in &self.data.terrain_tiles[1..] {
            let distance = candidate.inner_distance(p);
            if distance > depth {
                (tile, depth) = (candidate, distance);
            }
        }
        let Some(property) = &self.data.property else {
            return tile.sample_split(p);
        };
        if depth >= PROPERTY_BLEND_M {
            return tile.sample_split(p);
        }
        let coarse = property.terrain.sample_split(p);
        if depth <= 0.0 {
            return coarse;
        }
        coarse.blend(
            tile.sample_split(p),
            depth,
            PROPERTY_BLEND_M,
            tile.inner_direction(p),
        )
    }
}

#[derive(Debug, PartialEq)]
pub struct CourseData {
    pub id: String,
    pub name: String,
    pub location: String,
    pub bake_version: String,
    pub source_hash: String,
    pub frame: Frame,
    pub climate: Climate,
    pub maintenance: Maintenance,
    pub irrigation: Irrigation,
    pub materials: Vec<GroundMaterial>,
    pub terrain_tiles: Vec<HeightGrid>,
    pub fine_tiles: Vec<HeightGrid>,
    pub surface_tiles: Vec<SurfaceGrid>,
    pub property: Option<Property>,
    pub regions: Vec<Region>,
    pub bunkers: Vec<Bunker>,
    pub bunker_faces: Vec<BunkerFace>,
    pub trees: Trees,
    pub walls: Vec<Wall>,
    pub rocks: Vec<Rock>,
    pub holes: Vec<Hole>,
    pub features: Features,
    pub grade_bound: Real,
    pub drawing: Drawing,
}

#[derive(Debug, PartialEq)]
pub struct Frame {
    pub axes: String,
}

#[derive(Debug, PartialEq)]
pub struct Property {
    pub terrain: HeightGrid,
    pub surface: SurfaceGrid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceId {
    Tile(usize),
    Property,
}

#[derive(Debug, PartialEq)]
pub struct GroundMaterial {
    pub id: String,
    pub kind: Surface,
    pub grass_height_m: Real,
    pub grass: String,
    pub soil: String,
    pub drainage: String,
}

#[derive(Debug, PartialEq)]
pub struct Region {
    pub id: String,
    pub legal: LegalKind,
    pub penalty_color: PenaltyColor,
    pub hole: i64,
    pub polygon: Vec<Vec2>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegalKind {
    PuttingGreen,
    TeeingArea,
    Bunker,
    OutOfBounds,
    LostBall,
    PenaltyArea,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PenaltyColor {
    #[default]
    Red,
    Yellow,
}

#[derive(Debug, PartialEq)]
pub struct Bunker {
    pub id: String,
    pub hole: i64,
    pub outline: Vec<Vec2>,
    pub floor_m: Real,
}

#[derive(Debug, PartialEq)]
pub struct BunkerFace {
    pub bunker: String,
    pub base: Vec<Vec3>,
    pub top: Vec<Vec3>,
}

#[derive(Debug, PartialEq)]
pub struct Trees {
    pub species: Vec<String>,
    pub instances: Vec<Tree>,
}

#[derive(Debug, PartialEq)]
pub struct Tree {
    pub id: String,
    pub species_index: u8,
    pub base: Vec3,
    pub height_m: Real,
    pub crown_radius_m: Real,
    pub crown_base_m: Real,
    pub trunk_radius_m: Real,
    pub density: Real,
    pub cone: bool,
}

#[derive(Debug, PartialEq)]
pub struct Wall {
    pub id: String,
    pub base: Vec<Vec3>,
    pub height_m: Real,
    pub width_m: Real,
}

#[derive(Debug, PartialEq)]
pub struct Rock {
    pub id: String,
    pub center: Vec3,
    pub radii: Vec3,
    pub yaw_rad: Real,
}

#[derive(Debug, PartialEq)]
pub struct Hole {
    pub number: i64,
    pub par: i64,
    pub name: String,
    pub card: String,
    pub tees: Vec<Tee>,
    pub green: Green,
    pub pin_zones: Vec<PinZone>,
    pub walk: Vec<Vec3>,
}

#[derive(Debug, PartialEq)]
pub struct Tee {
    pub id: String,
    pub difficulty: i64,
    pub position: Vec3,
    pub aim: Vec3,
    pub pad_m: Vec2,
    pub length_yd: i64,
    pub play_line: Vec<Vec3>,
}

#[derive(Debug, PartialEq)]
pub struct Green {
    pub center: Vec3,
    pub outline: Vec<Vec2>,
    pub fine_tile: usize,
}

#[derive(Debug, PartialEq)]
pub struct PinZone {
    pub id: String,
    pub position: String,
    pub difficulty: i64,
    pub polygon: Vec<Vec2>,
    pub cups: Vec<Cup>,
}

#[derive(Debug, PartialEq)]
pub struct Cup {
    pub id: String,
    pub position: Vec3,
}

#[derive(Debug, PartialEq)]
pub struct Features {
    pub clubhouse: Clubhouse,
}

#[derive(Debug, PartialEq)]
pub struct Clubhouse {
    pub position: Vec3,
    pub size_m: Vec2,
    pub yaw_rad: Real,
}

#[derive(Debug, PartialEq)]
pub struct Drawing {
    pub turf_tiles: Variant,
    pub forest: Option<Variant>,
    pub wall_kit: Option<Variant>,
    pub wall_sections: Option<Variant>,
    pub wall_stones: Option<Variant>,
}

#[derive(Debug, PartialEq)]
pub struct Climate {
    pub altitude_m: Option<Real>,
    pub latitude_deg: Option<Real>,
    pub coastal: Option<bool>,
    pub sea_bearing_deg: Option<Real>,
    pub season: Option<Season>,
    pub monthly: Vec<MonthlyClimate>,
}

#[derive(Debug, PartialEq)]
pub struct Season {
    pub open: Option<String>,
    pub close: Option<String>,
    pub fall_colour_peak: Option<[String; 2]>,
}

#[derive(Debug, PartialEq)]
pub struct MonthlyClimate {
    pub month: Option<Real>,
    pub temp_c: Option<[Real; 2]>,
    pub humidity: Option<Real>,
    pub rain_days: Option<Real>,
    pub precip_mm: Option<Real>,
    pub wind_from_deg: Option<Real>,
    pub wind_mps: Option<Real>,
}

#[derive(Debug, PartialEq)]
pub struct Maintenance {
    pub everyday: Option<MaintenanceSetup>,
    pub tournament: Option<MaintenanceSetup>,
    pub edges: BTreeMap<String, GrassEdge>,
}

#[derive(Debug, PartialEq)]
pub struct MaintenanceSetup {
    pub green_speed_m: Option<[Real; 2]>,
    pub mowing_mm: BTreeMap<String, Real>,
    pub divot_repair: Option<String>,
    pub bunker_raking: Option<String>,
    pub irrigation_et_replaced: Option<Real>,
    pub greens_rolled: Option<bool>,
}

#[derive(Debug, PartialEq)]
pub struct GrassEdge {
    pub band_m: Option<Real>,
    pub low: Option<Real>,
    pub wander_m: Option<Real>,
    pub wander_step_m: Option<Real>,
    pub straggle_m: Option<Real>,
    pub clumps: Option<Real>,
    pub clump_m: Option<Real>,
    pub into: Option<Vec<String>>,
    pub clumps_into: Option<Vec<String>>,
}

#[derive(Debug, PartialEq)]
pub struct Irrigation {
    pub schedule: Option<IrrigationSchedule>,
    pub zones: Vec<IrrigationZone>,
}

#[derive(Debug, PartialEq)]
pub struct IrrigationSchedule {
    pub start: Option<String>,
    pub end: Option<String>,
}

#[derive(Debug, PartialEq)]
pub struct IrrigationZone {
    pub id: String,
    pub materials: Vec<String>,
    pub mm_per_night: BTreeMap<String, Real>,
}
