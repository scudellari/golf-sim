//! Constant-day hole setup and the player's saved settings.

use std::collections::BTreeMap;

use golf_sim_base::{
    ClubId, Error, Real, Seed, Vec3, fm,
    num::{deg_to_rad, lerpf},
    variant::{Array, Components, Dict, FloatWidth, Variant},
};
use golf_sim_conditions::{grounds::CourseGround, world::World};
use golf_sim_course::course::{Course, Cup, Tee};
use golf_sim_physics::{plane::ConstantAir, samplers::AirSample};
use golf_sim_stroke::{
    address::{Address, Assists, Preset},
    bag::Group,
};
use golf_sim_swing::spec::StrikeMode;

use crate::records::hole::Fields;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Mode {
    #[default]
    Caddie,
    Tour,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum StrikeVariant {
    A,
    B,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaySetting {
    StrikeVariant,
    InfoMode,
    Pace,
    TourSlopes,
    HoleOverview,
}

impl PlaySetting {
    const FIELDS: [(Self, &'static str); 5] = [
        (Self::StrikeVariant, "strike_variant"),
        (Self::InfoMode, "info_mode"),
        (Self::Pace, "pace"),
        (Self::TourSlopes, "tour_slopes"),
        (Self::HoleOverview, "hole_overview"),
    ];
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaySettings {
    pub strike_variant: StrikeVariant,
    pub info_mode: Mode,
    pub pace: Real,
    pub tour_slopes: bool,
    pub hole_overview: bool,
}

impl Default for PlaySettings {
    fn default() -> Self {
        Self {
            strike_variant: StrikeVariant::A,
            info_mode: Mode::Caddie,
            pace: 1.5,
            tour_slopes: false,
            hole_overview: true,
        }
    }
}

impl PlaySettings {
    pub fn window_scale(self) -> Real {
        match self.info_mode {
            Mode::Caddie => 1.2,
            Mode::Tour => 1.0,
        }
    }

    pub fn shows_slopes(self) -> bool {
        self.info_mode == Mode::Caddie || self.tour_slopes
    }

    pub fn assists(self) -> Assists {
        Assists {
            window_scale: self.window_scale(),
            strike_mode: match self.strike_variant {
                StrikeVariant::A => StrikeMode::Reticle,
                StrikeVariant::B => StrikeMode::None,
            },
        }
    }

    pub fn apply(&mut self, setting: PlaySetting, value: Real) -> bool {
        if !value.is_finite() {
            return false;
        }
        match setting {
            PlaySetting::StrikeVariant => {
                let Some(variant) = enum_choice(value, [StrikeVariant::A, StrikeVariant::B]) else {
                    return false;
                };
                self.strike_variant = variant;
            }
            PlaySetting::InfoMode => {
                let Some(mode) = enum_choice(value, [Mode::Caddie, Mode::Tour]) else {
                    return false;
                };
                self.info_mode = mode;
            }
            PlaySetting::Pace => self.pace = value.clamp(1.0, 2.0),
            PlaySetting::TourSlopes => self.tour_slopes = value >= 0.5,
            PlaySetting::HoleOverview => self.hole_overview = value >= 0.5,
        }
        true
    }

    pub fn to_dict(self) -> Dict {
        dictionary([
            ("strike_variant", (self.strike_variant as i64).into()),
            ("info_mode", (self.info_mode as i64).into()),
            ("pace", self.pace.into()),
            ("tour_slopes", Variant::Bool(self.tour_slopes)),
            ("hole_overview", Variant::Bool(self.hole_overview)),
        ])
    }

    pub fn from_dict(dict: &Dict) -> Self {
        let mut settings = Self::default();
        for (setting, key) in PlaySetting::FIELDS {
            let value = match dict.get(key) {
                Some(Variant::Bool(value)) => Some(Real::from(u8::from(*value))),
                Some(value) => number(value),
                None => None,
            };
            if let Some(value) = value {
                settings.apply(setting, value);
            }
        }
        settings
    }
}

fn enum_choice<T: Copy>(value: Real, choices: [T; 2]) -> Option<T> {
    usize::try_from(value as i64)
        .ok()
        .and_then(|index| choices.get(index).copied())
}

const PROFILE_FORMAT: &str = "links-lab-profile";
const PROFILE_VERSION: i64 = 1;
const PRESETS_PER_GROUP: usize = 8;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Profile {
    pub presets: Vec<Preset>,
    pub stock: BTreeMap<ClubId, Address>,
    pub settings: PlaySettings,
    pub starter_choice_made: bool,
    pub range_choices: BTreeMap<String, i64>,
    pub problems: Vec<String>,
}

impl Profile {
    pub fn presets_for(&self, group: Group) -> impl Iterator<Item = &Preset> {
        self.presets
            .iter()
            .filter(move |preset| preset.group == group)
            .take(PRESETS_PER_GROUP)
    }

    pub fn stock_for(&self, club: ClubId) -> Address {
        self.stock
            .get(&club)
            .map_or_else(|| Address::stock(club), |address| address.clone().fit(club))
    }

    pub fn to_dict(&self) -> Dict {
        let stock = self
            .stock
            .iter()
            .map(|(club, address)| (club.name().into(), Variant::Dict(address.to_dict())));
        let range = self
            .range_choices
            .iter()
            .map(|(name, &choice)| (name.as_str().into(), choice.into()));
        dictionary([
            ("format", PROFILE_FORMAT.into()),
            ("version", PROFILE_VERSION.into()),
            (
                "presets",
                Variant::Array(Array::new(
                    self.presets
                        .iter()
                        .map(|preset| Variant::Dict(preset.to_dict()))
                        .collect(),
                )),
            ),
            ("stock", Variant::Dict(Dict::new(stock.collect()))),
            ("settings", Variant::Dict(self.settings.to_dict())),
            (
                "starter_choice_made",
                Variant::Bool(self.starter_choice_made),
            ),
            ("range", Variant::Dict(Dict::new(range.collect()))),
        ])
    }

    pub fn from_dict(dict: &Dict) -> Self {
        let mut profile = Self::default();
        if dict.get("format").and_then(Variant::as_str) != Some(PROFILE_FORMAT) {
            profile.problems.push("not a profile file".into());
            return profile;
        }
        let version = dict.get("version").and_then(number);
        if !version.is_some_and(|version| version == PROFILE_VERSION as Real) {
            profile
                .problems
                .push(format!("unsupported profile version {version:?}"));
            return profile;
        }

        match dict.get("presets") {
            Some(Variant::Array(presets)) => {
                for entry in &presets.items {
                    if let Some(preset) = entry.as_dict().and_then(Preset::from_dict) {
                        profile.presets.push(preset);
                    } else {
                        profile.problems.push("skipped an unreadable preset".into());
                    }
                }
            }
            Some(_) => profile.problems.push("presets: expected Array".into()),
            None => {}
        }
        if let Some(stock) = profile_section(dict, "stock", &mut profile.problems) {
            for (club, address) in stock.entries() {
                match (club.as_str().and_then(ClubId::from_name), address.as_dict()) {
                    (Some(club), Some(address)) => {
                        profile.stock.insert(club, Address::from_dict(address));
                    }
                    _ => profile
                        .problems
                        .push(format!("invalid Stock entry for {club:?}")),
                }
            }
        }
        if let Some(settings) = profile_section(dict, "settings", &mut profile.problems) {
            profile.settings = PlaySettings::from_dict(settings);
        }
        profile.starter_choice_made =
            matches!(dict.get("starter_choice_made"), Some(Variant::Bool(true)));
        if let Some(range) = profile_section(dict, "range", &mut profile.problems) {
            for (key, value) in range.entries() {
                if let (Some(key), Some(choice)) = (key.as_str(), choice_index(value)) {
                    profile.range_choices.insert(key.into(), choice);
                }
            }
        }
        profile
    }
}

fn profile_section<'a>(dict: &'a Dict, key: &str, problems: &mut Vec<String>) -> Option<&'a Dict> {
    let value = dict.get(key)?;
    let section = value.as_dict();
    if section.is_none() {
        problems.push(format!("{key}: expected Dictionary"));
    }
    section
}

fn choice_index(value: &Variant) -> Option<i64> {
    match value {
        Variant::Int(value) => Some(value.value()),
        Variant::Float(value) if value.value().is_finite() => Some(value.value() as i64),
        _ => None,
    }
}

fn number(value: &Variant) -> Option<Real> {
    match value {
        Variant::Int(value) => Some(value.value() as Real),
        Variant::Float(value) => Some(value.value()),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HoleSetup {
    pub course_id: String,
    pub course_revision: String,
    pub hole: i64,
    pub tee_id: String,
    pub pin_zone: String,
    pub cup_id: String,
    pub seed: Seed,
    pub wind_mps: Vec3,
    pub tee_time_s: Real,
    pub temperature_c: Real,
    pub pressure_pa: Real,
    pub humidity: Real,
    pub firmness: Real,
    pub moisture: Real,
    pub green_speed_m: Real,
    pub settings: PlaySettings,
}

impl HoleSetup {
    pub fn standard(course: &Course, hole: i64, seed: Seed) -> Self {
        let data = course.data();
        let zones = course.hole(hole).map_or(&[][..], |hole| &hole.pin_zones);
        let zone = zones
            .iter()
            .find(|zone| zone.position == "middle")
            .or_else(|| zones.first());
        let mut setup = Self {
            course_id: data.id.clone(),
            course_revision: course.revision().into(),
            hole,
            tee_id: "middle".into(),
            pin_zone: zone.map_or_else(String::new, |zone| zone.id.clone()),
            cup_id: zone
                .and_then(|zone| zone.cups.first())
                .map_or_else(String::new, |cup| cup.id.clone()),
            seed,
            wind_mps: Vec3::ZERO,
            tee_time_s: 9.0 * 3600.0,
            temperature_c: 15.0,
            pressure_pa: World::station_pressure_pa(data.climate.altitude_m.unwrap_or_default()),
            humidity: 0.5,
            firmness: 0.5,
            moisture: 0.3,
            green_speed_m: 3.05,
            settings: PlaySettings::default(),
        };
        for month in &data.climate.monthly {
            if month.month.is_some_and(|number| number as i64 == 7) {
                let [low, high] = month.temp_c.unwrap_or([15.0; 2]);
                setup.temperature_c = lerpf(low, high, 0.7);
                setup.humidity = month.humidity.unwrap_or(0.5);
                let from = deg_to_rad(month.wind_from_deg.unwrap_or_default());
                setup.wind_mps = Vec3::new(-fm::sin(from), 0.0, fm::cos(from))
                    * month.wind_mps.unwrap_or_default();
            }
        }
        if let Some([low, high]) = data
            .maintenance
            .everyday
            .as_ref()
            .and_then(|maintenance| maintenance.green_speed_m)
        {
            setup.green_speed_m = 0.5 * (low + high);
        }
        setup
    }

    pub fn tee<'a>(&self, course: &'a Course) -> Option<&'a Tee> {
        course
            .hole(self.hole)?
            .tees
            .iter()
            .find(|tee| tee.id == self.tee_id)
    }

    pub fn cup<'a>(&self, course: &'a Course) -> Option<&'a Cup> {
        course
            .hole(self.hole)?
            .pin_zones
            .iter()
            .find(|zone| zone.id == self.pin_zone)?
            .cups
            .iter()
            .find(|cup| cup.id == self.cup_id)
    }

    pub fn air(&self) -> ConstantAir {
        ConstantAir::new(AirSample {
            temperature_c: self.temperature_c,
            pressure_pa: self.pressure_pa,
            relative_humidity: self.humidity,
            mean_wind_mps: self.wind_mps,
            ..AirSample::default()
        })
    }

    pub fn world<'a>(&self, course: &'a Course, clock_s: Real) -> Option<World<'a>> {
        if course.revision() != self.course_revision || course.data().id != self.course_id {
            return None;
        }
        self.tee(course)?;
        let cup = self.cup(course)?.position;
        let ground = CourseGround::new(course, self.firmness, self.moisture, self.green_speed_m);
        Some(World::on_course(ground, self.air(), Some(cup), clock_s))
    }

    /// The setup dictionary in the record. Its enclosing record stores the revision and settings.
    pub fn to_dict(&self) -> Dict {
        dictionary([
            ("course_id", self.course_id.as_str().into()),
            ("hole", self.hole.into()),
            ("tee_id", self.tee_id.as_str().into()),
            ("pin_zone", self.pin_zone.as_str().into()),
            ("cup_id", self.cup_id.as_str().into()),
            ("seed", self.seed.0.into()),
            ("wind_mps", vector3(self.wind_mps)),
            ("tee_time_s", self.tee_time_s.into()),
            ("temperature_c", self.temperature_c.into()),
            ("pressure_pa", self.pressure_pa.into()),
            ("humidity", self.humidity.into()),
            ("firmness", self.firmness.into()),
            ("moisture", self.moisture.into()),
            ("green_speed_m", self.green_speed_m.into()),
        ])
    }

    pub fn from_dict(
        dict: &Dict,
        course_revision: String,
        settings: PlaySettings,
    ) -> Result<Self, Error> {
        let fields = Fields::new(dict, "setup");
        Ok(Self {
            course_id: fields.string("course_id")?.into(),
            course_revision,
            hole: fields.integer("hole")?,
            tee_id: fields.string("tee_id")?.into(),
            pin_zone: fields.string("pin_zone")?.into(),
            cup_id: fields.string("cup_id")?.into(),
            seed: Seed(fields.integer("seed")?),
            wind_mps: fields.vector3("wind_mps")?,
            tee_time_s: fields.real("tee_time_s")?,
            temperature_c: fields.real("temperature_c")?,
            pressure_pa: fields.real("pressure_pa")?,
            humidity: fields.real("humidity")?,
            firmness: fields.real("firmness")?,
            moisture: fields.real("moisture")?,
            green_speed_m: fields.real("green_speed_m")?,
            settings,
        })
    }
}

pub(crate) fn dictionary<'a>(fields: impl IntoIterator<Item = (&'a str, Variant)>) -> Dict {
    Dict::new(
        fields
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .collect(),
    )
}

pub(crate) fn vector3(point: Vec3) -> Variant {
    Variant::Vector3(Components {
        values: [point.x, point.y, point.z],
        width: FloatWidth::Double,
    })
}
