//! Absolute address settings, Stock, and the intent and presets that produce an address.

use golf_sim_base::{
    ClubId, Real, Vec2,
    num::{clampf, deg_to_rad},
    variant::{Dict, Variant},
};
use golf_sim_course::legal::StanceFootprint;
use golf_sim_physics::{facade::standard_club, values::units::INCH};
use golf_sim_swing::spec::StrikeMode;

use crate::{
    bag::{self, Group},
    tuning as t,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Setting {
    Stance,
    BallPosition,
    Face,
    Hands,
    GripDown,
    SwingLength,
    TeeHeight,
}

impl Setting {
    pub const ALL: [Self; 7] = [
        Self::Stance,
        Self::BallPosition,
        Self::Face,
        Self::Hands,
        Self::GripDown,
        Self::SwingLength,
        Self::TeeHeight,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::Stance => "stance_open_rad",
            Self::BallPosition => "ball_forward_m",
            Self::Face => "face_open_rad",
            Self::Hands => "hands_forward_rad",
            Self::GripDown => "grip_down_m",
            Self::SwingLength => "swing_length",
            Self::TeeHeight => "tee_height_m",
        }
    }

    pub fn step_size(self) -> Real {
        t::SETTINGS[self as usize].step
    }

    pub fn big_step(self) -> i32 {
        t::SETTINGS[self as usize].big_step
    }

    pub fn range(self, club: ClubId) -> [Real; 2] {
        t::setting_range(bag::group(club), self)
    }

    fn value_mut(self, address: &mut Address) -> &mut Real {
        match self {
            Self::Stance => &mut address.stance_open_rad,
            Self::BallPosition => &mut address.ball_forward_m,
            Self::Face => &mut address.face_open_rad,
            Self::Hands => &mut address.hands_forward_rad,
            Self::GripDown => &mut address.grip_down_m,
            Self::SwingLength => &mut address.swing_length,
            Self::TeeHeight => &mut address.tee_height_m,
        }
    }
}

/// Angles are relative to the aim line. For a right-handed golfer an open stance aims
/// left, an open face aims right, and positive shaft lean puts the hands ahead of the ball.
#[derive(Clone, Debug, PartialEq)]
pub struct Address {
    pub stance_open_rad: Real,
    pub ball_forward_m: Real,
    pub face_open_rad: Real,
    pub hands_forward_rad: Real,
    pub grip_down_m: Real,
    pub swing_length: Real,
    pub tee_height_m: Real,
    pub clamped: Vec<Setting>,
}

impl Default for Address {
    fn default() -> Self {
        Self {
            stance_open_rad: 0.0,
            ball_forward_m: 0.0,
            face_open_rad: 0.0,
            hands_forward_rad: 0.0,
            grip_down_m: 0.0,
            swing_length: 1.0,
            tee_height_m: 0.0,
            clamped: Vec::new(),
        }
    }
}

impl Address {
    pub fn stock(club: ClubId) -> Self {
        let row = t::club(club);
        Self {
            ball_forward_m: row.ball_widths * t::BALL_WIDTH_M,
            hands_forward_rad: deg_to_rad(row.hands_deg),
            tee_height_m: row.tee_in * INCH,
            ..Self::default()
        }
    }

    pub fn from_intent(club: ClubId, intent: Intent, base: Option<&Self>) -> Self {
        let mut address = base.cloned().unwrap_or_else(|| Self::stock(club));
        address.swing_length = intent.swing_length;
        if club != ClubId::Putter {
            let shape = Real::from(intent.shape) * t::SHAPE_STANCE_RAD;
            let height = Real::from(intent.height);
            address.stance_open_rad += shape;
            address.face_open_rad -= shape * t::club(club).shape_face;
            address.ball_forward_m += height * t::HEIGHT_BALL_M;
            address.hands_forward_rad += height * t::HEIGHT_HANDS_RAD;
        }
        address.fit(club)
    }

    pub fn from_preset(club: ClubId, preset: &Preset) -> Self {
        preset.address.clone().fit(club)
    }

    pub fn value(&self, setting: Setting) -> Real {
        match setting {
            Setting::Stance => self.stance_open_rad,
            Setting::BallPosition => self.ball_forward_m,
            Setting::Face => self.face_open_rad,
            Setting::Hands => self.hands_forward_rad,
            Setting::GripDown => self.grip_down_m,
            Setting::SwingLength => self.swing_length,
            Setting::TeeHeight => self.tee_height_m,
        }
    }

    pub fn with_value(mut self, setting: Setting, value: Real) -> Self {
        *setting.value_mut(&mut self) = value;
        self
    }

    pub fn same_as(&self, other: &Self) -> bool {
        Setting::ALL
            .into_iter()
            .all(|setting| approximately_equal(self.value(setting), other.value(setting)))
    }

    pub fn fit(mut self, club: ClubId) -> Self {
        self.clamped.clear();
        for setting in Setting::ALL {
            let [min, max] = setting.range(club);
            let value = setting.value_mut(&mut self);
            let held = clampf(*value, min, max);
            if !approximately_equal(held, *value) {
                *value = held;
                self.clamped.push(setting);
            }
        }
        self
    }

    pub fn adjust(mut self, club: ClubId, setting: Setting, steps: i32) -> Self {
        *setting.value_mut(&mut self) += Real::from(steps) * setting.step_size();
        self.fit(club)
    }

    pub fn to_dict(&self) -> Dict {
        Dict::new(
            Setting::ALL
                .into_iter()
                .map(|setting| (setting.key().into(), self.value(setting).into()))
                .collect(),
        )
    }

    pub fn from_dict(dict: &Dict) -> Self {
        let mut address = Self::default();
        for setting in Setting::ALL {
            if let Some(value) = dict.get(setting.key()).and_then(finite_number) {
                *setting.value_mut(&mut address) = value;
            }
        }
        address
    }

    pub fn footprint(&self, club: ClubId) -> StanceFootprint {
        let width = t::club(club).stance_m;
        let away = if club == ClubId::Putter {
            t::PUTTER_STANCE_AWAY_M
        } else {
            t::STANCE_REACH_FRACTION * (standard_club(club).length_m - self.grip_down_m)
                - t::STANCE_REACH_OFFSET_M
        };
        let line = Vec2::new(1.0, 0.0).rotated(-self.stance_open_rad);
        let centre = Vec2::new(0.0, -away) - line * self.ball_forward_m;
        let half_width = line * width * 0.5;
        StanceFootprint {
            lead_foot_m: Vec2::new(centre.x + half_width.x, centre.y + half_width.y),
            trail_foot_m: centre - half_width,
            club_entry_m: Vec2::new(t::CLUB_ENTRY_ALONG_M, 0.0),
        }
    }
}

// Presets and clamping use the address editor's comparison, including its tolerance near zero.
fn approximately_equal(a: Real, b: Real) -> bool {
    a == b || (a - b).abs() < (1e-5 * a.abs()).max(1e-5)
}

fn finite_number(value: &Variant) -> Option<Real> {
    let value = match value {
        Variant::Int(value) => value.value() as Real,
        Variant::Float(value) => value.value(),
        _ => return None,
    };
    value.is_finite().then_some(value)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Intent {
    shape: i32,
    height: i32,
    pub swing_length: Real,
}

impl Intent {
    /// Negative shape draws, positive shape fades. Negative height lowers the flight.
    pub fn new(shape: i32, height: i32, swing_length: Real) -> Self {
        Self {
            shape: shape.clamp(-t::INTENT_STEPS, t::INTENT_STEPS),
            height: height.clamp(-t::INTENT_STEPS, t::INTENT_STEPS),
            swing_length,
        }
    }

    pub fn shape(self) -> i32 {
        self.shape
    }

    pub fn height(self) -> i32 {
        self.height
    }
}

impl Default for Intent {
    fn default() -> Self {
        Self::new(0, 0, 1.0)
    }
}

/// A named address template for one group. It stores absolute settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    pub name: String,
    pub group: Group,
    pub address: Address,
}

impl Preset {
    pub fn new(name: impl Into<String>, group: Group, mut address: Address) -> Self {
        address.clamped.clear();
        Self {
            name: name.into(),
            group,
            address,
        }
    }

    pub fn to_dict(&self) -> Dict {
        Dict::new(vec![
            ("name".into(), self.name.as_str().into()),
            ("group".into(), (self.group as i64).into()),
            ("address".into(), Variant::Dict(self.address.to_dict())),
        ])
    }

    pub fn from_dict(dict: &Dict) -> Option<Self> {
        let name = dict.get("name")?.as_str()?;
        let group = finite_number(dict.get("group")?)? as i64;
        let group = *Group::ALL.get(usize::try_from(group).ok()?)?;
        let address = Address::from_dict(dict.get("address")?.as_dict()?);
        Some(Self::new(name, group, address))
    }

    pub fn starters() -> [Self; 7] {
        use ClubId::{Driver, Iron7, SandWedge};
        use Setting::{BallPosition, Face, GripDown, Hands, SwingLength, TeeHeight};

        fn starter(name: &str, club: ClubId, steps: &[(Setting, i32)]) -> Preset {
            let address = steps
                .iter()
                .fold(Address::stock(club), |address, &(setting, step)| {
                    address.adjust(club, setting, step)
                });
            Preset::new(name, bag::group(club), address)
        }

        [
            starter(
                "Fairway finder",
                Driver,
                &[(GripDown, 2), (SwingLength, -2), (TeeHeight, -1)],
            ),
            starter(
                "Punch",
                Iron7,
                &[
                    (BallPosition, -4),
                    (Hands, 4),
                    (GripDown, 1),
                    (SwingLength, -5),
                ],
            ),
            starter("High soft", Iron7, &[(BallPosition, 2), (Hands, -3)]),
            starter(
                "Chip",
                SandWedge,
                &[
                    (BallPosition, -3),
                    (Hands, 4),
                    (GripDown, 2),
                    (SwingLength, -15),
                ],
            ),
            starter("Pitch", SandWedge, &[(SwingLength, -10)]),
            starter(
                "Flop",
                SandWedge,
                &[
                    (BallPosition, 3),
                    (Face, 15),
                    (Hands, -5),
                    (SwingLength, -5),
                ],
            ),
            starter(
                "Bunker splash",
                SandWedge,
                &[
                    (BallPosition, 5),
                    (Face, 10),
                    (Hands, -4),
                    (SwingLength, -6),
                ],
            ),
        ]
    }
}

/// Player settings used to size the swing meter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Assists {
    pub window_scale: Real,
    pub strike_mode: StrikeMode,
}

impl Default for Assists {
    fn default() -> Self {
        Self {
            window_scale: 1.0,
            strike_mode: StrikeMode::Reticle,
        }
    }
}
