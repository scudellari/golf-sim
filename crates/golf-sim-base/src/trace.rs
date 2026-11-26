//! Stroke probes and traces.

use serde::{Deserialize, Serialize, Serializer};

/// A phase of a stroke. Each bounce starts a separate phase entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Delivery,
    Impact,
    Launch,
    Flight,
    Bounce,
    Skid,
    Roll,
    Cup,
    Lie,
    Ruling,
}

/// Where a recorded number came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    Tunable { name: String },
    Turf { name: String },
    Course { name: String },
    SeedDraw { name: String },
    Computed,
}

/// Receives the phases and diagnostic values of a stroke.
pub trait Probe {
    /// Guard calculations and allocations made solely for the trace with this constant.
    const ENABLED: bool;

    /// Starts a new entry, even when the same phase was just recorded.
    fn begin_phase(&mut self, phase: Phase);

    /// Records a number in the current phase. Call `begin_phase` first.
    fn value(&mut self, name: &str, value: f64, unit: &str, source: Source);
}

/// Discards trace events.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoProbe;

impl Probe for NoProbe {
    const ENABLED: bool = false;

    #[inline]
    fn begin_phase(&mut self, _: Phase) {}

    #[inline]
    fn value(&mut self, _: &str, _: f64, _: &str, _: Source) {}
}

/// An ordered record of one stroke. Serialize it with `serde_json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Trace {
    phases: Vec<PhaseTrace>,
}

impl Trace {
    /// Returns the phases in the order they were recorded.
    pub fn phases(&self) -> &[PhaseTrace] {
        &self.phases
    }
}

impl Probe for Trace {
    const ENABLED: bool = true;

    fn begin_phase(&mut self, phase: Phase) {
        self.phases.push(PhaseTrace {
            phase,
            values: Vec::new(),
        });
    }

    fn value(&mut self, name: &str, value: f64, unit: &str, source: Source) {
        self.phases
            .last_mut()
            .expect("begin a phase before recording a trace value")
            .values
            .push(TraceValue {
                name: name.to_owned(),
                value,
                unit: unit.to_owned(),
                source,
            });
    }
}

/// One occurrence of a phase, with its values in recording order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PhaseTrace {
    pub phase: Phase,
    pub values: Vec<TraceValue>,
}

/// A named number, its unit, and its source.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TraceValue {
    pub name: String,
    #[serde(serialize_with = "serialize_number")]
    pub value: f64,
    pub unit: String,
    pub source: Source,
}

fn serialize_number<S: Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
    if value.is_finite() {
        serializer.serialize_f64(*value)
    } else {
        // JSON would otherwise serialize a non-finite float as null, which cannot round-trip.
        Err(serde::ser::Error::custom("trace values must be finite"))
    }
}
