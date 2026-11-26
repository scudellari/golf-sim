use std::{collections::BTreeMap, fmt};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
pub struct TargetFile {
    pub format: u32,
    pub source: String,
    pub citation: String,
    pub url: String,
    pub datasets: Vec<String>,
    pub verdict: crate::Verdict,
    pub conditions: Conditions,
    #[serde(deserialize_with = "read_targets")]
    pub targets: Vec<Target>,
}

fn read_targets<'de, D: serde::Deserializer<'de>>(de: D) -> Result<Vec<Target>, D::Error> {
    Vec::<serde_json::Value>::deserialize(de)?
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            let label = match value.get("id").and_then(serde_json::Value::as_str) {
                Some(id) => format!("target {id:?}"),
                None => format!("targets[{index}]"),
            };
            let target: Target = serde_json::from_value(value)
                .map_err(|error| serde::de::Error::custom(format!("{label}: {error}")))?;
            target
                .validate()
                .map_err(|error| serde::de::Error::custom(format!("{label}: {error}")))?;
            Ok(target)
        })
        .collect()
}

#[derive(Clone, Debug, Deserialize)]
pub struct Conditions {
    pub air_temp_c: f64,
    pub pressure_pa: f64,
    pub humidity: f64,
    pub wind_mph: f64,
    pub ball: String,
    pub lie: String,
    pub ground: Ground,
    pub green: Green,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Ground {
    pub surface: String,
    pub firmness: f64,
    pub moisture: f64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Green {
    pub stimp_ft: f64,
    pub firmness: f64,
    pub moisture: f64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Target {
    pub id: String,
    pub kind: Kind,
    pub submodel: Submodel,
    #[serde(rename = "use")]
    pub role: Role,
    #[serde(default)]
    pub held_out: Vec<String>,
    pub inputs: Quantities,
    pub measured: Quantities,
    #[serde(default)]
    pub tolerance: BTreeMap<String, Tolerance>,
    #[serde(default)]
    pub check: BTreeMap<String, Check>,
    pub earlier: Option<Earlier>,
    pub note: Option<String>,
}

impl Target {
    fn validate(&self) -> Result<(), String> {
        if self.id.is_empty() || self.measured.is_empty() {
            return Err("a target requires an id and at least one measurement".into());
        }
        for (name, tolerance) in &self.tolerance {
            if !matches!(self.measured.get(name), Some(Quantity::Number(_))) || !tolerance.valid() {
                return Err(format!("invalid source tolerance for {name:?}"));
            }
        }
        for name in &self.held_out {
            if !self.measured.contains_key(name) {
                return Err(format!("held-out quantity {name:?} has no measurement"));
            }
        }
        if let Some(earlier) = &self.earlier {
            for (name, prediction) in &earlier.predicted {
                if !self.measured.get(name).is_some_and(|measured| {
                    std::mem::discriminant(measured) == std::mem::discriminant(prediction)
                }) {
                    return Err(format!(
                        "earlier prediction for {name:?} has no matching measurement"
                    ));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn role_for(&self, name: &str) -> Role {
        if self.held_out.iter().any(|quantity| quantity == name) {
            Role::HeldOut
        } else {
            self.role
        }
    }
}

pub type Quantities = BTreeMap<String, Quantity>;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Quantity {
    Number(f64),
    Bool(bool),
    Text(String),
}

impl Quantity {
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Self::Number(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Text(value) => Some(value),
            _ => None,
        }
    }
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(value) => fmt::Display::fmt(value, f),
            Self::Bool(value) => fmt::Display::fmt(value, f),
            Self::Text(value) => write!(f, "{value:?}"),
        }
    }
}

macro_rules! named_enum {
    ($name:ident { $($variant:ident => $label:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
        pub enum $name {
            $(#[serde(rename = $label)] $variant),+
        }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $label),+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

pub(crate) use named_enum;

named_enum!(Kind {
    Launch => "launch",
    Delivery => "delivery",
    TurfImpact => "turf_impact",
    RunOut => "run_out",
    GreenSpeed => "green_speed",
    StockCarry => "stock_carry",
    CupCapture => "cup_capture",
    Aero => "aero",
});

named_enum!(Submodel {
    Flight => "flight",
    Impact => "impact",
    TurfBounce => "turf_bounce",
    RunoutGreens => "runout_greens",
    Cup => "cup",
    Stroke => "stroke",
});

named_enum!(Role { Fit => "fit", HeldOut => "held-out" });
named_enum!(ToleranceKind { Sd => "sd", Ci95 => "ci95", Range => "range" });

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tolerance {
    Sd(f64),
    Ci95(f64),
    Range([f64; 2]),
}

impl Tolerance {
    pub fn kind(self) -> ToleranceKind {
        match self {
            Self::Sd(_) => ToleranceKind::Sd,
            Self::Ci95(_) => ToleranceKind::Ci95,
            Self::Range(_) => ToleranceKind::Range,
        }
    }

    /// Half the width of the accepted band, in the measurement's units.
    pub fn half_width(self) -> f64 {
        match self {
            Self::Sd(value) => 2.0 * value,
            Self::Ci95(value) => value,
            Self::Range([lo, hi]) => 0.5 * hi - 0.5 * lo,
        }
    }

    fn valid(self) -> bool {
        let width = self.half_width();
        width.is_finite() && width >= 0.0 && !matches!(self, Self::Range([lo, hi]) if lo > hi)
    }

    pub(crate) fn contains(self, prediction: f64, error: f64) -> bool {
        match self {
            Self::Range([lo, hi]) => (lo..=hi).contains(&prediction),
            _ => error <= self.half_width(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Check {
    Abs(f64),
    Rel(f64),
}

named_enum!(EarlierVia { Production => "production", FitModel => "fit model" });

#[derive(Clone, Debug, Deserialize)]
pub struct Earlier {
    pub via: EarlierVia,
    pub predicted: Quantities,
    pub residual: BTreeMap<String, f64>,
}
