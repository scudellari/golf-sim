use golf_sim_base::{
    Real,
    digest::object,
    trace::{Phase, Probe, Source},
    variant::{Array, Dict, Variant},
};
use golf_sim_physics::{cup, tunables, values::EventKind};

use crate::tolerance::CaseOutcome;

use super::values::{Version, recorded_tunables, version};

#[derive(Clone, Copy)]
pub(in crate::corpus) struct Versions {
    recorded: Option<Version>,
    current: Version,
}

impl Versions {
    pub(in crate::corpus) fn for_fixture(header: &Dict) -> Self {
        Self {
            recorded: recorded_tunables(header).and_then(version),
            current: version(tunables::VERSION).expect("physics model version"),
        }
    }

    fn predates(self, model: Version) -> bool {
        self.recorded.is_some_and(|earlier| earlier < model) && model <= self.current
    }

    pub(in crate::corpus) fn run(
        self,
        run: impl FnOnce(&mut ModelProbe) -> Result<Variant, String>,
    ) -> Result<CaseOutcome, String> {
        let mut probe = ModelProbe::default();
        let actual = run(&mut probe)?;
        Ok(self.classify(probe, actual))
    }

    fn classify(self, probe: ModelProbe, actual: Variant) -> CaseOutcome {
        let changes = [
            (probe.flagstick, (0, 9, 0), Response::Flagstick),
            (probe.slope, (0, 10, 0), Response::SlopeCapture),
            (probe.green, (0, 11, 0), Response::GreenBounce),
        ];
        let first = changes
            .into_iter()
            .filter(|(_, version, _)| self.predates(*version))
            .filter_map(|(time, _, response)| time.map(|time| (time, response)))
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((time, response)) = first {
            CaseOutcome::ModelDeparture {
                actual,
                first: AffectedEvent { time, response },
                change: response.change(),
            }
        } else {
            actual.into()
        }
    }
}

#[derive(Clone, Copy)]
enum Response {
    Flagstick,
    SlopeCapture,
    GreenBounce,
}

impl Response {
    fn change(self) -> &'static str {
        match self {
            Self::Flagstick => "physics 0.9.0: flagstick rim response",
            Self::SlopeCapture => "physics 0.10.0: cup capture on slopes",
            Self::GreenBounce => "physics 0.11.0: green bounce across the iron range",
        }
    }

    fn matches(self, kind: i64) -> bool {
        match self {
            Self::Flagstick => kind == EventKind::Obstacle as i64,
            Self::SlopeCapture => kind == EventKind::CupEntry as i64,
            Self::GreenBounce => {
                [EventKind::Bounce as i64, EventKind::RollStart as i64].contains(&kind)
            }
        }
    }
}

pub(crate) struct AffectedEvent {
    time: Real,
    response: Response,
}

impl AffectedEvent {
    pub(crate) fn unaffected(&self, output: &Variant) -> Option<Variant> {
        let output = output.as_dict()?;
        if let Some(Variant::Dict(shot)) = output.get("playback") {
            let (playback, touches, pitches) = self.shot_prefix(shot)?;
            return Some(transform(output, &[], |key, value| match key {
                "playback" => playback.clone(),
                "outcome" => {
                    let fields = value.as_dict().expect("stroke outcome");
                    let mut ignored = vec![
                        "end",
                        "rest",
                        "rest_surface",
                        "total_m",
                        "embedded",
                        "penalty_entry",
                    ];
                    if touches == 0 {
                        ignored.push("carry_m");
                    }
                    transform(fields, &ignored, |key, value| {
                        if let ("marks", Variant::Array(marks)) = (key, value) {
                            let divot_count = marks
                                .items
                                .first()
                                .and_then(Variant::as_dict)
                                .and_then(|mark| int(mark, "kind"))
                                .is_some_and(|kind| kind == 0);
                            Variant::Array(Array::new(
                                marks
                                    .items
                                    .iter()
                                    .take(usize::from(divot_count) + pitches)
                                    .cloned()
                                    .collect(),
                            ))
                        } else {
                            value.clone()
                        }
                    })
                }
                _ => value.clone(),
            }));
        }
        self.shot_prefix(output).map(|(shot, _, _)| shot)
    }

    fn shot_prefix(&self, shot: &Dict) -> Option<(Variant, usize, usize)> {
        let Variant::Array(events) = shot.get("events")? else {
            return None;
        };
        let Variant::Array(touchdowns) = shot.get("touchdowns")? else {
            return None;
        };
        let Variant::PackedFloat64Array(times) = shot.get("times")? else {
            return None;
        };
        let Variant::PackedVector3Array(positions) = shot.get("positions")? else {
            return None;
        };
        let (index, cutoff) = events
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, event)| {
                let event = event.as_dict()?;
                let time = number(event, "time_s")?;
                self.response
                    .matches(int(event, "kind")?)
                    .then_some((index, time))
            })
            .min_by(|a, b| (a.1 - self.time).abs().total_cmp(&(b.1 - self.time).abs()))?;
        if (cutoff - self.time).abs() > 1e-3 {
            return None;
        }
        let first_response = events.items[index].as_dict()?;
        let changed: &[&str] = match self.response {
            Response::Flagstick => &["velocity", "spin"],
            Response::SlopeCapture => &[],
            Response::GreenBounce => &["kind", "velocity", "spin"],
        };
        let mut prefix_events = events.items[..index].to_vec();
        prefix_events.push(transform(first_response, changed, |_, value| value.clone()));
        let prefix_contacts: Vec<_> = touchdowns
            .items
            .iter()
            .take_while(|value| {
                value
                    .as_dict()
                    .and_then(|contact| number(contact, "time_s"))
                    .is_some_and(|t| t <= cutoff)
            })
            .cloned()
            .collect();
        let pitches = prefix_contacts
            .iter()
            .filter(|v| {
                v.as_dict()
                    .and_then(|contact| number(contact, "depth_m"))
                    .is_some_and(|d| d > 0.0)
            })
            .count();
        let count = prefix_contacts.len();
        let prefix_samples = times.partition_point(|&t| t < cutoff);
        let mut positions = positions.clone();
        positions.values.truncate(prefix_samples);
        let prefix = transform(
            shot,
            &["end_reason", "rest_position", "rest_surface", "embedded"],
            |key, value| match key {
                "events" => Variant::Array(Array::new(prefix_events.clone())),
                "touchdowns" => Variant::Array(Array::new(prefix_contacts.clone())),
                "times" => Variant::PackedFloat64Array(times[..prefix_samples].to_vec()),
                "positions" => Variant::PackedVector3Array(positions.clone()),
                _ => value.clone(),
            },
        );
        Some((prefix, count, pitches))
    }
}

fn int(fields: &Dict, key: &str) -> Option<i64> {
    let Variant::Int(n) = fields.get(key)? else {
        return None;
    };
    Some(n.value())
}

fn number(fields: &Dict, key: &str) -> Option<Real> {
    let Variant::Float(n) = fields.get(key)? else {
        return None;
    };
    Some(n.value())
}

fn transform(dict: &Dict, ignored: &[&str], map: impl Fn(&str, &Variant) -> Variant) -> Variant {
    object(dict.entries().iter().filter_map(|(key, value)| {
        let key = key.as_str()?;
        (!ignored.contains(&key)).then(|| (key, map(key, value)))
    }))
}

#[derive(Default)]
pub(in crate::corpus) struct ModelProbe {
    phase: Option<Phase>,
    flagstick: Option<Real>,
    slope: Option<Real>,
    green: Option<Real>,
    rim_speed: Real,
    lateral: Real,
    crossing_to_stick: bool,
    changed_capture: bool,
    loaded_green: bool,
}

impl ModelProbe {
    fn observe(&mut self, name: &str, value: Real) {
        match (self.phase, name) {
            (Some(Phase::Bounce), "iron_landing_load") => self.loaded_green = value > 0.0,
            (Some(Phase::Cup), "rim_speed") => self.rim_speed = value,
            (Some(Phase::Cup), "crossing_to_flagstick") => self.crossing_to_stick = true,
            (Some(Phase::Cup), "lateral_offset") => self.lateral = value,
            (Some(Phase::Cup), "capture_speed") if !self.crossing_to_stick => {
                let flat = cup::putt_capture_speed(self.lateral);
                self.changed_capture = (self.rim_speed <= flat) != (self.rim_speed <= value);
            }
            _ => {}
        }
    }
}

impl Probe for ModelProbe {
    const ENABLED: bool = true;

    fn begin_phase(&mut self, phase: Phase) {
        self.phase = Some(phase);
        self.crossing_to_stick = false;
        self.changed_capture = false;
        self.loaded_green = false;
    }

    fn value(&mut self, name: &str, value: Real, _: &str, _: Source) {
        self.observe(name, value);
    }

    fn boundary(&mut self, name: &'static str, time: Real) {
        match name {
            "terrain_contact_s" if self.loaded_green => {
                self.green.get_or_insert(time);
            }
            "ground_flagstick_s" => {
                self.flagstick.get_or_insert(time);
            }
            "cup_entry_s" if self.changed_capture => {
                self.slope.get_or_insert(time);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_real_ground_flagstick_contact_marks_only_an_older_fixture() {
        use golf_sim_base::{Surface, Vec3};
        use golf_sim_physics::{
            integrator::{self, Options},
            plane::{ConstantAir, PlaneGround},
            samplers::{AirSample, GroundSample},
            values::{Cup, Launch, LaunchReadout},
        };
        let ground = PlaneGround::new(
            GroundSample {
                surface: Surface::Green,
                ..GroundSample::default()
            },
            Vec3::ZERO,
        );
        let position = Vec3::new(0.0, 0.0, tunables::CUP_RADIUS_M + 0.001);
        let launch = Launch::from_readout(
            &tunables::BALL,
            position,
            Vec3::new(0.0, 0.0, -1.0),
            LaunchReadout {
                ball_speed_mps: 2.0,
                ..LaunchReadout::default()
            },
        );
        let run = |recorded, flagstick_in| {
            Versions {
                recorded: Some(recorded),
                current: (0, 11, 0),
            }
            .run(|probe| {
                integrator::run(
                    &launch,
                    &tunables::BALL,
                    &ConstantAir::new(AirSample::default()),
                    &ground,
                    Some(Cup {
                        position: Vec3::ZERO,
                        flagstick_in,
                    }),
                    Options::default(),
                    probe,
                );
                Ok(Variant::Nil)
            })
            .unwrap()
        };
        assert!(
            matches!(run((0, 8, 0), true), CaseOutcome::ModelDeparture { change, .. }
            if change == "physics 0.9.0: flagstick rim response")
        );
        assert!(matches!(run((0, 9, 0), true), CaseOutcome::Compared { .. }));
        assert!(matches!(
            run((0, 8, 0), false),
            CaseOutcome::Compared { .. }
        ));
    }

    #[test]
    fn a_changed_bounce_keeps_apex_and_incoming_contact_strict() {
        let marker = AffectedEvent {
            time: 3.0,
            response: Response::GreenBounce,
        };
        let snapshot = |apex: Real, incoming: Real, outgoing: Real| {
            let event = |kind, time: Real, speed| {
                object([
                    ("kind", (kind as i64).into()),
                    ("time_s", time.into()),
                    ("velocity", Variant::from(speed)),
                    ("position", 2.0.into()),
                ])
            };
            object([
                (
                    "events",
                    Variant::Array(Array::new(vec![
                        event(EventKind::Apex, 1.5, apex),
                        event(EventKind::Bounce, 3.0, outgoing),
                        event(EventKind::Rest, 4.0, 0.0),
                    ])),
                ),
                (
                    "touchdowns",
                    Variant::Array(Array::new(vec![object([
                        ("time_s", 3.0.into()),
                        ("velocity", incoming.into()),
                        ("depth_m", 0.0.into()),
                    ])])),
                ),
                ("times", Variant::PackedFloat64Array(vec![])),
                (
                    "positions",
                    Variant::PackedVector3Array(golf_sim_base::variant::Components {
                        values: vec![],
                        width: golf_sim_base::variant::FloatWidth::Double,
                    }),
                ),
                ("rest_position", outgoing.into()),
            ])
        };
        let reference = marker.unaffected(&snapshot(1.0, 2.0, 3.0)).unwrap();
        assert_eq!(
            marker.unaffected(&snapshot(1.0, 2.0, 4.0)).unwrap(),
            reference
        );
        assert_ne!(
            marker.unaffected(&snapshot(1.1, 2.0, 4.0)).unwrap(),
            reference
        );
        assert_ne!(
            marker.unaffected(&snapshot(1.0, 2.1, 4.0)).unwrap(),
            reference
        );
    }

    #[test]
    fn model_departures_require_a_changed_path_and_an_older_fixture() {
        let run = |recorded, green| {
            Versions {
                recorded,
                current: (0, 11, 0),
            }
            .run(|probe| {
                probe.begin_phase(Phase::Bounce);
                probe.value("iron_landing_load", green, "", Source::Computed);
                probe.boundary("terrain_contact_s", 1.5);
                Ok(Variant::Nil)
            })
            .unwrap()
        };
        assert!(matches!(
            run(Some((0, 8, 0)), 1.0),
            CaseOutcome::ModelDeparture { change, .. } if change.contains("green bounce")
        ));
        for (recorded, load) in [(None, 1.0), (Some((0, 11, 0)), 1.0), (Some((0, 8, 0)), 0.0)] {
            assert!(matches!(run(recorded, load), CaseOutcome::Compared { .. }));
        }
    }
}
