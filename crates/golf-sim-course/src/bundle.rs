use std::{collections::BTreeMap, path::Path, sync::Arc};

use golf_sim_base::{
    Error, Real, Surface, Vec2, Vec3,
    container::Manifest,
    digest::sha256_hex,
    variant::{self, Components, Dict, FloatWidth, Variant},
};

use crate::{
    course::{
        Bunker, BunkerFace, Climate, Clubhouse, Course, CourseData, Cup, Drawing, Features, Frame,
        GrassEdge, Green, GroundMaterial, Hole, Irrigation, IrrigationSchedule, IrrigationZone,
        LegalKind, Maintenance, MaintenanceSetup, MonthlyClimate, PenaltyColor, PinZone, Property,
        Region, Rock, Season, Tee, Tree, Trees, Wall,
    },
    grid::{GridFrame, HeightGrid, SurfaceGrid},
};

const FORMAT: &str = "links-lab-course";
const FORMAT_VERSION: i64 = 1;
const BAKE_VERSION: &str = "0.6.0";

pub(crate) fn open(dir: &Path) -> Result<Arc<Course>, Error> {
    let manifest = Manifest::read(dir)?;
    require_version("manifest.json.format", &manifest.format, FORMAT)?;
    require_version(
        "manifest.json.bake_version",
        &manifest.bake_version,
        BAKE_VERSION,
    )?;
    let bytes = manifest.read_payload(dir)?;
    let data = parse_data(&variant::decode(&bytes)?)?;
    if data.id != manifest.course_id {
        return Err(malformed(
            "course.id",
            "differs from manifest.json.course_id",
        ));
    }
    Ok(Course::new(manifest.course_revision, data))
}

pub(crate) fn parse(bytes: &[u8]) -> Result<Arc<Course>, Error> {
    let data = parse_data(&variant::decode(bytes)?)?;
    Ok(Course::new(sha256_hex(bytes), data))
}

fn require_version(path: &str, value: &str, supported: &str) -> Result<(), Error> {
    if value != supported {
        return Err(malformed(
            path,
            format!("unsupported value {value:?}, expected {supported:?}"),
        ));
    }
    Ok(())
}

fn parse_data(value: &Variant) -> Result<CourseData, Error> {
    let root = Field {
        value,
        path: "$".into(),
    };
    require_version("format", &root.get::<String>("format")?, FORMAT)?;
    let version = root.get("format_version")?;
    if version != FORMAT_VERSION {
        return Err(Error::UnknownVersion {
            what: "format_version",
            version,
        });
    }
    let bake_version = root.get::<String>("bake_version")?;
    require_version("bake_version", &bake_version, BAKE_VERSION)?;

    let course = root.key("course")?;
    let materials = root.get::<Vec<GroundMaterial>>("materials")?;
    let terrain_tiles = root.get::<Vec<HeightGrid>>("terrain_tiles")?;
    let fine_tiles = root.get::<Vec<HeightGrid>>("fine_tiles")?;
    let surface_tiles = root.key("surface_tiles")?;
    let surface_tiles = surface_tiles.list(|tile| surface(tile, materials.len()))?;
    for (key, empty) in [
        ("terrain_tiles", terrain_tiles.is_empty()),
        ("surface_tiles", surface_tiles.is_empty()),
    ] {
        if empty {
            return Err(malformed(key, "expected at least one tile"));
        }
    }
    let property = root
        .optional_key("property")?
        .map(|field| {
            Ok(Property {
                terrain: field.get("terrain")?,
                surface: surface(field.key("surface")?, materials.len())?,
            })
        })
        .transpose()?;
    let holes = root
        .key("holes")?
        .list(|field| hole(field, fine_tiles.len()))?;
    let grade_bound = root.get::<Real>("grade_bound")?;
    if grade_bound < 0.0 {
        return Err(malformed("grade_bound", "expected a nonnegative slope"));
    }

    Ok(CourseData {
        id: course.get("id")?,
        name: course.get("name")?,
        location: course.get("location")?,
        bake_version,
        source_hash: root.get("source_hash")?,
        frame: root.get("frame")?,
        climate: root.get("climate")?,
        maintenance: root.get("maintenance")?,
        irrigation: root.get("irrigation")?,
        materials,
        terrain_tiles,
        fine_tiles,
        surface_tiles,
        property,
        regions: root.get("regions")?,
        bunkers: root.get("bunkers")?,
        bunker_faces: root.get("bunker_faces")?,
        walls: root.get("walls")?,
        rocks: root.get("rocks")?,
        trees: root.get("trees")?,
        holes,
        features: root.get("features")?,
        grade_bound,
        drawing: Drawing {
            turf_tiles: root.key("turf_tiles")?.keep("Array", is_array)?,
            forest: root.keep_optional("forest", "Dictionary", |v| v.as_dict().is_some())?,
            wall_kit: root
                .keep_optional("wall_kit", "String", |v| matches!(v, Variant::String(_)))?,
            wall_sections: root.keep_optional("wall_sections", "Array", is_array)?,
            wall_stones: root.keep_optional("wall_stones", "Array", is_array)?,
        },
    })
}

fn is_array(value: &Variant) -> bool {
    matches!(value, Variant::Array(_))
}

fn grid_frame(
    field: &Field<'_>,
    minimum_size: i32,
) -> Result<(GridFrame, [Vec2; 2], usize), Error> {
    let origin_field = field.key("origin")?;
    let Variant::Vector2(stored_origin) = origin_field.value else {
        return Err(origin_field.wrong("Vector2"));
    };
    let width = stored_origin.width;
    let origin = Vec2::read(origin_field)?;
    let cell_m = field.get::<Real>("cell_m")?;
    let size = field.get::<[i32; 2]>("size")?;
    if cell_m <= 0.0 {
        return Err(field.invalid("cell_m", "expected positive cell size"));
    }
    if size.iter().any(|&axis| axis < minimum_size) {
        return Err(field.invalid(
            "size",
            format!("expected at least {minimum_size} nodes on each axis"),
        ));
    }
    let size = size.map(|axis| axis as usize);
    let count = size[0]
        .checked_mul(size[1])
        .ok_or_else(|| field.invalid("size", "node count is too large"))?;

    // Tile selection changes the split-height datum at blend boundaries. Form the
    // rectangles in the stored frame's vector precision before widening for queries.
    let vector = |x, y| frame_vector(x, y, width);
    let step = vector(cell_m, cell_m)?.x;
    let nodes = vector((size[0] - 1) as Real, (size[1] - 1) as Real)?;
    let span = vector(nodes.x * step, nodes.y * step)?;
    let end = vector(origin.x + span.x, origin.y + span.y)?;
    if !end.x.is_finite() || !end.y.is_finite() || end.x <= origin.x || end.y <= origin.y {
        return Err(field.invalid("size", "expected a finite, positive grid span"));
    }
    let start = vector(origin.x + step, origin.y + step)?;
    let inset = vector(span.x - 2.0 * step, span.y - 2.0 * step)?;
    let inner_end = vector(start.x + inset.x, start.y + inset.y)?;
    Ok((
        GridFrame::new(origin, cell_m, size, end),
        [start, inner_end],
        count,
    ))
}

fn frame_vector(x: Real, y: Real, width: FloatWidth) -> Result<Vec2, Error> {
    if width == FloatWidth::Double {
        return Ok(Vec2::new(x, y));
    }
    let bytes = variant::encode(&Variant::Vector2(Components {
        values: [x, y],
        width,
    }))?;
    let Variant::Vector2(vector) = variant::decode(&bytes)? else {
        unreachable!("encoded a Vector2");
    };
    Ok(Vec2::new(vector.values[0], vector.values[1]))
}

impl Read for HeightGrid {
    fn read(field: Field<'_>) -> Result<Self, Error> {
        let (frame, inner, count) = grid_frame(&field, 4)?;
        let heights = field.get::<Vec<Real>>("heights")?;
        if heights.len() != count {
            return Err(field.invalid(
                "heights",
                format!("expected {count} samples, got {}", heights.len()),
            ));
        }
        Ok(Self::new(frame, heights, inner))
    }
}

fn surface(field: Field<'_>, material_count: usize) -> Result<SurfaceGrid, Error> {
    let (frame, _, count) = grid_frame(&field, 2)?;
    let materials = field.get::<Vec<u8>>("materials")?;
    if materials.len() != count {
        return Err(field.invalid(
            "materials",
            format!("expected {count} samples, got {}", materials.len()),
        ));
    }
    if let Some(index) = materials
        .iter()
        .position(|&id| usize::from(id) >= material_count)
    {
        return Err(malformed(
            format!("{}.materials[{index}]", field.path),
            "material index out of range",
        ));
    }
    let stand = field.optional::<Vec<u8>>("stand")?.unwrap_or_default();
    if !stand.is_empty() && stand.len() != count {
        return Err(field.invalid(
            "stand",
            format!("expected {count} samples, got {}", stand.len()),
        ));
    }
    Ok(SurfaceGrid::new(frame, materials, stand))
}

impl Read for GroundMaterial {
    fn read(field: Field<'_>) -> Result<Self, Error> {
        Ok(Self {
            id: field.get("id")?,
            kind: field.get("kind")?,
            grass_height_m: field.get("grass_height_m")?,
            grass_density: field.optional("grass_density")?.unwrap_or(-1.0),
            grass: field.get("grass")?,
            soil: field.get("soil")?,
            drainage: field.get("drainage")?,
        })
    }
}

macro_rules! read_enum {
    ($ty:ty { $($name:literal => $variant:ident),+ $(,)? }) => {
        impl Read for $ty {
            fn read(field: Field<'_>) -> Result<Self, Error> {
                match field.string()? {
                    $($name => Ok(Self::$variant),)+
                    value => Err(malformed(&field.path, format!("unknown value {value:?}"))),
                }
            }
        }
    };
}

impl Read for Surface {
    fn read(field: Field<'_>) -> Result<Self, Error> {
        let value = field.string()?;
        Self::from_name(value)
            .ok_or_else(|| malformed(field.path, format!("unknown surface {value:?}")))
    }
}

read_enum!(LegalKind {
    "putting_green" => PuttingGreen,
    "teeing_area" => TeeingArea,
    "bunker" => Bunker,
    "out_of_bounds" => OutOfBounds,
    "lost_ball" => LostBall,
    "penalty_area" => PenaltyArea,
});
read_enum!(PenaltyColor { "red" => Red, "yellow" => Yellow });

impl Read for Region {
    fn read(field: Field<'_>) -> Result<Self, Error> {
        Ok(Self {
            id: field.get("id")?,
            legal: field.get("legal")?,
            penalty_color: field.optional("penalty_color")?.unwrap_or_default(),
            hole: field.get("hole")?,
            polygon: field.key("polygon")?.polygon()?,
        })
    }
}

impl Read for Bunker {
    fn read(field: Field<'_>) -> Result<Self, Error> {
        Ok(Self {
            id: field.get("id")?,
            hole: field.get("hole")?,
            outline: field.key("outline")?.polygon()?,
            floor_m: field.get("floor_m")?,
        })
    }
}

impl Read for BunkerFace {
    fn read(field: Field<'_>) -> Result<Self, Error> {
        let base = field.key("base")?.line()?;
        let top = field.key("top")?.line()?;
        if base.len() != top.len() {
            return Err(field.invalid("top", "point count differs from base"));
        }
        Ok(Self {
            bunker: field.get("bunker")?,
            base,
            top,
        })
    }
}

impl Read for Trees {
    fn read(field: Field<'_>) -> Result<Self, Error> {
        let species = field.get::<Vec<String>>("species")?;
        let ids = field.key("id")?;
        let Variant::PackedStringArray(ids) = ids.value else {
            return Err(ids.wrong("PackedStringArray"));
        };
        let count = ids.len();
        let species_index: Vec<u8> = field.column("species_index", count)?;
        let base: Vec<Vec3> = field.column("base", count)?;
        let heights: Vec<Real> = field.column("height_m", count)?;
        let crown_radii: Vec<Real> = field.column("crown_radius_m", count)?;
        let crown_bases: Vec<Real> = field.column("crown_base_m", count)?;
        let trunk_radii: Vec<Real> = field.column("trunk_radius_m", count)?;
        let density: Vec<Real> = field.column("density", count)?;
        let cone: Vec<u8> = field.column("cone", count)?;
        let instances = ids
            .iter()
            .enumerate()
            .map(|(i, id)| {
                if usize::from(species_index[i]) >= species.len() {
                    return Err(malformed(
                        format!("{}.species_index[{i}]", field.path),
                        "species index out of range",
                    ));
                }
                if cone[i] > 1 {
                    return Err(malformed(
                        format!("{}.cone[{i}]", field.path),
                        "expected 0 or 1",
                    ));
                }
                Ok(Tree {
                    id: id.clone(),
                    species_index: species_index[i],
                    base: base[i],
                    height_m: heights[i],
                    crown_radius_m: crown_radii[i],
                    crown_base_m: crown_bases[i],
                    trunk_radius_m: trunk_radii[i],
                    density: density[i],
                    cone: cone[i] == 1,
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { species, instances })
    }
}

impl Read for Wall {
    fn read(field: Field<'_>) -> Result<Self, Error> {
        Ok(Self {
            id: field.get("id")?,
            base: field.key("base")?.line()?,
            height_m: field.get("height_m")?,
            width_m: field.get("width_m")?,
        })
    }
}

impl Read for Rock {
    fn read(field: Field<'_>) -> Result<Self, Error> {
        Ok(Self {
            id: field.get("id")?,
            center: field.get("center")?,
            radii: field.get("radii")?,
            yaw_rad: field.get("yaw_rad")?,
        })
    }
}

fn hole(field: Field<'_>, fine_count: usize) -> Result<Hole, Error> {
    let number = field.get("number")?;
    let par = field.get("par")?;
    let name = field.get("name")?;
    let lines = field.key("play_lines")?.map(Field::line)?;
    let tees = field.key("tees")?.list(|tee| {
        let id = tee.get::<String>("id")?;
        let play_line = lines.get(&id).cloned().ok_or_else(|| Error::MissingKey {
            path: format!("{}.play_lines.{id}", field.path),
        })?;
        Ok(Tee {
            id,
            difficulty: tee.get("difficulty")?,
            position: tee.get("position")?,
            aim: tee.get("aim")?,
            pad_m: tee.get("pad_m")?,
            length_yd: tee.get("length_yd")?,
            play_line,
        })
    })?;
    if tees.is_empty() {
        return Err(field.invalid("tees", "expected at least one tee"));
    }

    let green = field.key("green")?;
    let fine_tile = usize::try_from(green.get::<i64>("fine_tile")?)
        .ok()
        .filter(|&index| index < fine_count)
        .ok_or_else(|| green.invalid("fine_tile", "fine tile index out of range"))?;
    let pin_zones = field.get::<Vec<PinZone>>("pin_zones")?;
    if !pin_zones.iter().any(|zone| !zone.cups.is_empty()) {
        return Err(field.invalid("pin_zones", "expected at least one cup"));
    }

    Ok(Hole {
        number,
        par,
        name,
        card: field.get("card")?,
        tees,
        green: Green {
            center: green.get("center")?,
            outline: green.key("outline")?.polygon()?,
            fine_tile,
        },
        pin_zones,
        walk: field.optional("walk")?.unwrap_or_default(),
    })
}

impl Read for PinZone {
    fn read(field: Field<'_>) -> Result<Self, Error> {
        Ok(Self {
            id: field.get("id")?,
            position: field.get("position")?,
            difficulty: field.get("difficulty")?,
            polygon: field.key("polygon")?.polygon()?,
            cups: field.get("cups")?,
        })
    }
}

macro_rules! read_fields {
    ($ty:ty { $($field:ident $($mode:ident)?),+ $(,)? }) => {
        impl Read for $ty {
            fn read(field: Field<'_>) -> Result<Self, Error> {
                Ok(Self {
                    $($field: read_fields!(@read field, stringify!($field) $(, $mode)?),)+
                })
            }
        }
    };
    (@read $field:ident, $key:expr) => { $field.get($key)? };
    (@read $field:ident, $key:expr, optional) => { $field.optional($key)? };
    (@read $field:ident, $key:expr, default) => { $field.optional($key)?.unwrap_or_default() };
}

read_fields!(Frame { axes });
read_fields!(Cup { id, position });
read_fields!(Features { clubhouse });
read_fields!(Clubhouse {
    position,
    size_m,
    yaw_rad
});
read_fields!(Climate {
    altitude_m optional, latitude_deg optional, coastal optional, sea_bearing_deg optional,
    season optional, monthly default,
});
read_fields!(Season { open optional, close optional, fall_colour_peak optional });
read_fields!(MonthlyClimate {
    month optional, temp_c optional, humidity optional, rain_days optional, precip_mm optional,
    wind_from_deg optional, wind_mps optional,
});
read_fields!(Maintenance { everyday optional, tournament optional, edges default });
read_fields!(MaintenanceSetup {
    green_speed_m optional, mowing_mm default, divot_repair optional, bunker_raking optional,
    irrigation_et_replaced optional, greens_rolled optional,
});
read_fields!(GrassEdge {
    band_m optional, low optional, wander_m optional, wander_step_m optional, straggle_m optional,
    clumps optional, clump_m optional, into optional, clumps_into optional,
});
read_fields!(Irrigation { schedule optional, zones default });
read_fields!(IrrigationSchedule { start optional, end optional });
read_fields!(IrrigationZone {
    id,
    materials,
    mm_per_night
});

trait Read: Sized {
    fn read(field: Field<'_>) -> Result<Self, Error>;

    fn read_list(field: Field<'_>) -> Result<Vec<Self>, Error> {
        field.list(Self::read)
    }
}

struct Field<'a> {
    value: &'a Variant,
    path: String,
}

impl<'a> Field<'a> {
    fn path_to(&self, key: &str) -> String {
        if self.path == "$" {
            key.into()
        } else {
            format!("{}.{key}", self.path)
        }
    }

    fn wrong(&self, expected: &'static str) -> Error {
        Error::WrongType {
            path: self.path.clone(),
            expected,
        }
    }

    fn invalid(&self, key: &str, message: impl Into<String>) -> Error {
        malformed(self.path_to(key), message)
    }

    fn dict(&self) -> Result<&'a Dict, Error> {
        self.value.as_dict().ok_or_else(|| self.wrong("Dictionary"))
    }

    fn string(&self) -> Result<&'a str, Error> {
        match self.value {
            Variant::String(value) => Ok(value),
            _ => Err(self.wrong("String")),
        }
    }

    fn key(&self, key: &str) -> Result<Self, Error> {
        self.optional_key(key)?.ok_or_else(|| Error::MissingKey {
            path: self.path_to(key),
        })
    }

    fn optional_key(&self, key: &str) -> Result<Option<Self>, Error> {
        Ok(self.dict()?.get(key).map(|value| Self {
            value,
            path: self.path_to(key),
        }))
    }

    fn get<T: Read>(&self, key: &str) -> Result<T, Error> {
        T::read(self.key(key)?)
    }

    fn optional<T: Read>(&self, key: &str) -> Result<Option<T>, Error> {
        self.optional_key(key)?.map(T::read).transpose()
    }

    fn keep(self, expected: &'static str, matches: fn(&Variant) -> bool) -> Result<Variant, Error> {
        if !matches(self.value) {
            return Err(self.wrong(expected));
        }
        Ok(self.value.clone())
    }

    fn keep_optional(
        &self,
        key: &str,
        expected: &'static str,
        matches: fn(&Variant) -> bool,
    ) -> Result<Option<Variant>, Error> {
        self.optional_key(key)?
            .map(|field| field.keep(expected, matches))
            .transpose()
    }

    fn list<T>(&self, read: impl Fn(Self) -> Result<T, Error>) -> Result<Vec<T>, Error> {
        let Variant::Array(array) = self.value else {
            return Err(self.wrong("Array"));
        };
        array
            .items
            .iter()
            .enumerate()
            .map(|(i, value)| {
                read(Self {
                    value,
                    path: format!("{}[{i}]", self.path),
                })
            })
            .collect()
    }

    fn map<T>(
        &self,
        read: impl Fn(Self) -> Result<T, Error>,
    ) -> Result<BTreeMap<String, T>, Error> {
        self.dict()?
            .entries()
            .iter()
            .map(|(key, value)| {
                let Variant::String(key) = key else {
                    return Err(self.wrong("Dictionary with String keys"));
                };
                Ok((
                    key.clone(),
                    read(Self {
                        value,
                        path: self.path_to(key),
                    })?,
                ))
            })
            .collect()
    }

    fn column<T>(&self, key: &str, count: usize) -> Result<Vec<T>, Error>
    where
        Vec<T>: Read,
    {
        let values = self.get::<Vec<T>>(key)?;
        if values.len() != count {
            return Err(self.invalid(
                key,
                format!("expected {count} entries, got {}", values.len()),
            ));
        }
        Ok(values)
    }

    fn polygon(self) -> Result<Vec<Vec2>, Error> {
        let path = self.path.clone();
        let points = Vec::<Vec2>::read(self)?;
        if points.len() < 3 {
            return Err(malformed(path, "expected at least three polygon points"));
        }
        Ok(points)
    }

    fn line(self) -> Result<Vec<Vec3>, Error> {
        let path = self.path.clone();
        let points = Vec::<Vec3>::read(self)?;
        if points.len() < 2 {
            return Err(malformed(path, "expected at least two line points"));
        }
        Ok(points)
    }
}

impl<T: Read> Read for Vec<T> {
    fn read(field: Field<'_>) -> Result<Self, Error> {
        T::read_list(field)
    }
}

impl<T: Read> Read for [T; 2] {
    fn read(field: Field<'_>) -> Result<Self, Error> {
        field
            .list(T::read)?
            .try_into()
            .map_err(|_| malformed(field.path, "expected exactly two entries"))
    }
}

impl<T: Read> Read for BTreeMap<String, T> {
    fn read(field: Field<'_>) -> Result<Self, Error> {
        field.map(T::read)
    }
}

macro_rules! read_variant {
    ($ty:ty, $variant:ident, $value:ident => $convert:expr) => {
        impl Read for $ty {
            fn read(field: Field<'_>) -> Result<Self, Error> {
                match field.value {
                    Variant::$variant($value) => Ok($convert),
                    _ => Err(field.wrong(stringify!($variant))),
                }
            }
        }
    };
}

read_variant!(i64, Int, value => value.value());
read_variant!(bool, Bool, value => *value);
read_variant!(String, String, value => value.clone());
read_variant!([i32; 2], Vector2i, value => *value);
read_variant!(Vec<u8>, PackedByteArray, value => value.clone());

impl Read for Real {
    fn read(field: Field<'_>) -> Result<Self, Error> {
        let Variant::Float(value) = field.value else {
            return Err(field.wrong("Float"));
        };
        let value = value.value();
        if !value.is_finite() {
            return Err(malformed(field.path, "expected a finite number"));
        }
        Ok(value)
    }

    fn read_list(field: Field<'_>) -> Result<Vec<Self>, Error> {
        let Variant::PackedFloat32Array(values) = field.value else {
            return Err(field.wrong("PackedFloat32Array"));
        };
        if let Some(index) = values.iter().position(|value| !value.is_finite()) {
            return Err(malformed(
                format!("{}[{index}]", field.path),
                "expected a finite number",
            ));
        }
        Ok(values.clone())
    }
}

macro_rules! read_vector {
    ($vec:ident, $variant:ident, $packed:ident, [$($component:ident),+]) => {
        impl Read for $vec {
            fn read(field: Field<'_>) -> Result<Self, Error> {
                let Variant::$variant(value) = field.value else {
                    return Err(field.wrong(stringify!($variant)));
                };
                let [$($component),+] = value.values;
                if !($($component.is_finite())&&+) {
                    return Err(malformed(field.path, "expected finite coordinates"));
                }
                Ok(Self::new($($component),+))
            }

            fn read_list(field: Field<'_>) -> Result<Vec<Self>, Error> {
                let Variant::$packed(values) = field.value else {
                    return Err(field.wrong(stringify!($packed)));
                };
                values.values.iter().enumerate().map(|(i, &[$($component),+])| {
                    if !($($component.is_finite())&&+) {
                        return Err(malformed(format!("{}[{i}]", field.path), "expected finite coordinates"));
                    }
                    Ok(Self::new($($component),+))
                }).collect()
            }
        }
    };
}

read_vector!(Vec2, Vector2, PackedVector2Array, [x, y]);
read_vector!(Vec3, Vector3, PackedVector3Array, [x, y, z]);

fn malformed(path: impl Into<String>, message: impl Into<String>) -> Error {
    Error::Malformed {
        path: path.into(),
        message: message.into(),
    }
}
