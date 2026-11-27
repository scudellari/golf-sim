//! Evidence for a periodic sample immediately before the rest event.

use golf_sim_base::{
    Real, Surface, fm,
    variant::{Dict, Variant},
};
use golf_sim_physics::{tunables as t, values::EventKind};

const STEP: Real = f32::EPSILON as Real;
const CLOCK_EPSILON: Real = 1e-9;
const TIME_BOUND: Real = 1e-6;

fn code(fields: &Dict, key: &str) -> Option<i64> {
    let Variant::Int(value) = fields.get(key)? else {
        return None;
    };
    Some(value.value())
}

fn time(event: &Dict) -> Option<Real> {
    let Variant::Float(value) = event.get("time_s")? else {
        return None;
    };
    Some(value.value()).filter(|time| time.is_finite())
}

fn array<'a>(fields: &'a Dict, key: &str) -> Option<&'a [Variant]> {
    let Variant::Array(value) = fields.get(key)? else {
        return None;
    };
    Some(&value.items)
}

struct Playback<'a> {
    shot: &'a Dict,
    times: &'a [Real],
    events: &'a [Variant],
}

impl<'a> Playback<'a> {
    fn read(shot: &'a Variant) -> Option<Self> {
        let shot = shot.as_dict()?;
        let Variant::PackedFloat64Array(times) = shot.get("times")? else {
            return None;
        };
        let Variant::PackedVector3Array(positions) = shot.get("positions")? else {
            return None;
        };
        if times.len() != positions.values.len() || times.len() < 3 {
            return None;
        }
        Some(Self {
            shot,
            times,
            events: array(shot, "events")?,
        })
    }

    fn roll_tail(&self) -> Option<(Real, Real)> {
        let rest = self.events.last()?.as_dict()?;
        let roll = self.events.iter().rev().nth(1)?.as_dict()?;
        let stop = time(rest)?;
        let start = time(roll)?;
        if code(rest, "kind")? != EventKind::Rest as i64
            || code(roll, "kind")? != EventKind::PureRoll as i64
            || code(rest, "surface")? != code(roll, "surface")?
            || code(rest, "surface")? != code(self.shot, "rest_surface")?
            || *self.times.last()? != stop
            || stop <= start
        {
            return None;
        }
        Some((start, stop))
    }
}

fn align_tick(
    expected: &Variant,
    actual: &Variant,
    step_scale: Real,
) -> Result<(Variant, Variant, Real), String> {
    let reference = Playback::read(expected).ok_or("reference playback is incomplete")?;
    let computed = Playback::read(actual).ok_or("core playback is incomplete")?;
    let (short, long, remove_expected) = match reference.times.len().cmp(&computed.times.len()) {
        std::cmp::Ordering::Less => (&reference, &computed, false),
        std::cmp::Ordering::Greater => (&computed, &reference, true),
        std::cmp::Ordering::Equal => return Err("no extra playback sample at rest".into()),
    };
    let fail = || "terminal updates do not bracket one extra playback tick".to_owned();
    if long.times.len() != short.times.len() + 1 {
        return Err(fail());
    }
    let (start_short, rest_short) = short.roll_tail().ok_or_else(fail)?;
    let (start_long, rest_long) = long.roll_tail().ok_or_else(fail)?;
    let surface = code(short.shot, "rest_surface").ok_or_else(fail)?;
    if code(long.shot, "rest_surface") != Some(surface) {
        return Err(fail());
    }
    let dt = step_scale
        * if [Surface::Green as i64, Surface::Fringe as i64].contains(&surface) {
            t::PUTTING_GROUND_DT
        } else {
            t::GROUND_DT
        };
    let period = t::SAMPLE_INTERVAL_S;
    let index = long.times.len() - 2;
    let extra = long.times[index];
    let before = rest_short - dt;
    let tick = fm::floor((extra + CLOCK_EPSILON) / period) * period;
    let on_roll_grid = |stamp: Real, start: Real| {
        let steps = (stamp - start) / dt;
        steps >= 0.0 && (stamp - (start + steps.round() * dt)).abs() <= CLOCK_EPSILON
    };
    let preceding = short.times[short.times.len() - 2];
    let preceding_long = long.times[index - 1];
    if !(dt > 0.0
        && (rest_long - extra - dt).abs() <= CLOCK_EPSILON
        && on_roll_grid(preceding, start_short)
        && on_roll_grid(rest_short, start_short)
        && on_roll_grid(preceding_long, start_long)
        && on_roll_grid(rest_long, start_long)
        && (tick - period..tick - period + dt).contains(&(preceding + CLOCK_EPSILON))
        && (tick - period..tick - period + dt).contains(&(preceding_long + CLOCK_EPSILON))
        && before + CLOCK_EPSILON < tick
        && tick <= extra + CLOCK_EPSILON
        && tick <= rest_short + CLOCK_EPSILON
        && extra < tick + dt
        && (rest_short - rest_long).abs() <= 1e-3)
    {
        return Err(fail());
    }
    let mut shorter = if remove_expected { expected } else { actual }.clone();
    let Variant::Dict(shot) = &mut shorter else {
        unreachable!("checked playback dictionary")
    };
    if let Some(Variant::PackedFloat64Array(times)) = shot.get_mut("times") {
        times.remove(index);
    }
    if let Some(Variant::PackedVector3Array(positions)) = shot.get_mut("positions") {
        positions.values.remove(index);
    }
    let (expected, actual) = if remove_expected {
        (shorter, actual.clone())
    } else {
        (expected.clone(), shorter)
    };
    Ok((expected, actual, tick))
}

fn response(kind: i64) -> Option<&'static str> {
    Some(match kind {
        k if k == EventKind::Landing as i64 => "contact",
        k if k == EventKind::Bounce as i64 => "bounce",
        k if k == EventKind::RollStart as i64 => "roll start",
        k if k == EventKind::PureRoll as i64 => "pure roll",
        k if k == EventKind::Obstacle as i64 => "obstacle contact",
        k if k == EventKind::Canopy as i64 => "canopy entry",
        k if k == EventKind::SurfaceEntered as i64 => "surface entry",
        k if k == EventKind::ZoneEntered as i64 => "zone entry",
        k if k == EventKind::CupEntry as i64 => "cup entry",
        k if k == EventKind::LipOut as i64 => "lip out",
        k if k == EventKind::Holed as i64 => "cup stop",
        k if k == EventKind::Rest as i64 => "stop",
        _ => return None,
    })
}

struct Checkpoint {
    path: String,
    a: Variant,
    b: Variant,
    time: Real,
    response: Option<&'static str>,
}

impl Checkpoint {
    fn field(&self, local: &str) -> String {
        if local == "$" {
            self.path.clone()
        } else {
            format!("{}.{local}", self.path)
        }
    }

    fn first_difference(&self) -> Option<String> {
        let local = golf_sim_base::corpus::first_difference(&self.a, &self.b)?;
        let detail = if let Some((a, b, bound)) = self.numeric(&local) {
            format!(" (delta {:.6e}, rounding bound {bound:.6e})", (a - b).abs())
        } else {
            " (discrete difference)".into()
        };
        Some(format!(
            "{} at {:.9} s{detail}",
            self.field(&local),
            self.time
        ))
    }

    fn numeric(&self, path: &str) -> Option<(Real, Real, Real)> {
        let (field, axis) = if matches!(self.a, Variant::Vector3(_)) {
            ("$", path)
        } else {
            path.split_once('.').unwrap_or((path, ""))
        };
        let get = |value: &Variant| -> Option<(Real, Real)> {
            let value = if field == "$" {
                value
            } else {
                value.as_dict()?.get(field)?
            };
            match value {
                Variant::Float(value) => Some((value.value(), value.value().abs())),
                Variant::Vector3(value) => {
                    let index = ["x", "y", "z"].iter().position(|key| *key == axis)?;
                    let scale = value
                        .values
                        .iter()
                        .copied()
                        .map(Real::abs)
                        .fold(1.0, Real::max);
                    Some((value.values[index], scale))
                }
                _ => None,
            }
        };
        let (a, scale_a) = get(&self.a)?;
        let (b, scale_b) = get(&self.b)?;
        let bound = if self.path.starts_with("times[") || field == "time_s" {
            TIME_BOUND
        } else {
            4.0 * STEP * scale_a.max(scale_b).max(1.0)
        };
        Some((a, b, bound))
    }

    fn rounding_difference(&self) -> Option<String> {
        golf_sim_base::corpus::first_difference_with(&self.a, &self.b, |path| {
            self.numeric(path)
                .map(|(_, _, bound)| golf_sim_base::corpus::Tolerance {
                    absolute: bound,
                    relative: 0.0,
                })
        })
    }
}

fn vector_variant(values: [Real; 3]) -> Variant {
    use golf_sim_base::variant::{Components, FloatWidth};
    Variant::Vector3(Components {
        values,
        width: FloatWidth::Double,
    })
}

fn state(shot: &Dict) -> Variant {
    let Some(launch) = shot.get("launch").and_then(Variant::as_dict) else {
        return Variant::Nil;
    };
    golf_sim_base::digest::object(
        ["position", "velocity", "spin", "aim", "valid", "contact"]
            .into_iter()
            .filter_map(|key| Some((key, launch.get(key)?.clone()))),
    )
}

fn checkpoints(a: &Dict, b: &Dict) -> Vec<Checkpoint> {
    let mut result = vec![Checkpoint {
        path: "launch".into(),
        a: state(a),
        b: state(b),
        time: 0.0,
        response: None,
    }];
    let mut responses = [Vec::new(), Vec::new()];
    for key in ["touchdowns", "events"] {
        let reference = array(a, key).unwrap_or_default();
        let computed = array(b, key).unwrap_or_default();
        for i in 0..reference.len().max(computed.len()) {
            let pair = [reference.get(i), computed.get(i)];
            let stamps = pair.map(|event| {
                let fields = event?.as_dict()?;
                let stamp = time(fields)?;
                let kind = if key == "touchdowns" {
                    Some("contact")
                } else {
                    code(fields, "kind").and_then(response)
                };
                Some((stamp, kind))
            });
            for (timeline, stamp) in responses.iter_mut().zip(stamps) {
                if let Some((stamp, Some(kind))) = stamp {
                    timeline.push((stamp, kind));
                }
            }
            // At equal clocks, either side may have emitted the response.
            let Some((time, direct)) = stamps.into_iter().flatten().min_by(|a, b| {
                a.0.total_cmp(&b.0)
                    .then_with(|| b.1.is_some().cmp(&a.1.is_some()))
            }) else {
                continue;
            };
            let [a, b] = pair.map(|value| value.cloned().unwrap_or(Variant::Nil));
            result.push(Checkpoint {
                path: format!("{key}[{i}]"),
                a,
                b,
                time,
                response: direct,
            });
        }
    }
    let history = |stamp: Real, window: Real| {
        responses
            .iter()
            .flatten()
            .filter(|(when, _)| stamp - *when >= 0.0 && stamp - *when <= window + CLOCK_EPSILON)
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, kind)| *kind)
    };
    for checkpoint in &mut result[1..] {
        checkpoint.response = checkpoint
            .response
            .or_else(|| history(checkpoint.time, t::FLIGHT_DT));
    }
    let samples = |shot: &Dict| -> Vec<(Real, [Real; 3])> {
        match (shot.get("times"), shot.get("positions")) {
            (
                Some(Variant::PackedFloat64Array(times)),
                Some(Variant::PackedVector3Array(positions)),
            ) => times
                .iter()
                .copied()
                .zip(positions.values.iter().copied())
                .collect(),
            _ => Vec::new(),
        }
    };
    let reference = samples(a);
    let computed = samples(b);
    for i in 0..reference.len().max(computed.len()) {
        let pair = [reference.get(i), computed.get(i)];
        let time = pair
            .into_iter()
            .flatten()
            .map(|(time, _)| *time)
            .fold(Real::INFINITY, Real::min);
        let response = history(time, t::SAMPLE_INTERVAL_S);
        let [a, b] = pair.map(|value| value.map_or(Variant::Nil, |(time, _)| (*time).into()));
        result.push(Checkpoint {
            path: format!("times[{i}]"),
            a,
            b,
            time,
            response,
        });
        let [a, b] =
            pair.map(|value| value.map_or(Variant::Nil, |(_, point)| vector_variant(*point)));
        result.push(Checkpoint {
            path: format!("positions[{i}]"),
            a,
            b,
            time,
            response,
        });
    }
    result.sort_by(|a, b| a.time.total_cmp(&b.time));
    result
}

pub(crate) struct Evidence {
    pub first: String,
    pub rounding: Result<(), String>,
    pub over_bound: String,
    pub threshold: Option<&'static str>,
}

pub(crate) fn evidence(
    expected: &Variant,
    raw: &Variant,
    compared: &Variant,
    limits: &crate::tolerance::Layer,
    delivered: Option<Result<(), String>>,
    step_scale: Real,
) -> Result<Evidence, String> {
    let shot = |value: &Variant| {
        value
            .as_dict()
            .is_some_and(|fields| fields.get("playback").is_some())
    };
    let prefix = if shot(expected) { "playback." } else { "" };
    let read = |value: &Variant| -> Option<()> {
        playback(value)?.as_dict()?;
        Some(())
    };
    for value in [expected, raw, compared] {
        read(value).ok_or("missing shot checkpoints")?;
    }
    let dict = |value| playback(value).unwrap().as_dict().unwrap();
    let raw_points = checkpoints(dict(expected), dict(raw));
    let first = raw_points.iter().find_map(|point| {
        point
            .first_difference()
            .map(|difference| (point.time, difference))
    });
    let first_time = first.as_ref().map(|(time, _)| *time);
    let first = first.map_or_else(
        || "no differing trajectory checkpoint".into(),
        |(_, value)| value,
    );
    let rounding = delivered.unwrap_or_else(|| {
        let time = first_time.ok_or("no differing trajectory checkpoint")?;
        let beyond = raw_points
            .iter()
            .take_while(|point| point.time <= time)
            .find_map(|point| Some(point.field(&point.rounding_difference()?)));
        beyond.map_or(Ok(()), |path| {
            Err(format!("first checkpoint exceeds float rounding at {path}"))
        })
    });

    let mut ordinary = checkpoints(dict(expected), dict(compared));
    // The raw first checkpoint uses the state propagated by flight. The normal
    // comparison also checks launch readouts and metadata before any response.
    include_launch_readouts(&mut ordinary, dict(expected), dict(compared));
    let early_fields = [
        ("fault", ""),
        ("quality", ""),
        ("fat", ""),
        ("start", "outcome"),
        ("ball_radius_m", "outcome"),
    ];
    if !prefix.is_empty() {
        for (key, section) in early_fields {
            let value = |output| field_at(output, section, key).unwrap_or(&Variant::Nil);
            let path = if section.is_empty() {
                key.into()
            } else {
                format!("{section}.{key}")
            };
            if limits
                .checkpoint_difference(value(expected), value(compared), &path)
                .is_some()
            {
                return Ok(Evidence {
                    first,
                    rounding,
                    over_bound: format!("{path} before flight"),
                    threshold: None,
                });
            }
        }
    }
    for point in &ordinary {
        let row = format!("{prefix}{}", point.path);
        let Some(local) = limits.checkpoint_difference(&point.a, &point.b, &row) else {
            continue;
        };
        let over_bound = format!("{} at {:.9} s", point.field(&local), point.time);
        // A length mismatch at the end can arise when only one run samples the last roll tick.
        if point.response == Some("stop")
            && point.path.starts_with("times[")
            && (point.a == Variant::Nil || point.b == Variant::Nil)
            && let Ok((aligned_a, aligned_b, _)) = align_tick(
                playback(expected).unwrap(),
                playback(compared).unwrap(),
                step_scale,
            )
            && limits
                .checkpoint_difference(&aligned_a, &aligned_b, prefix.trim_end_matches('.'))
                .is_none()
        {
            return Ok(Evidence {
                first,
                rounding,
                over_bound,
                threshold: Some("stop playback tick"),
            });
        }
        return Ok(Evidence {
            first,
            rounding,
            over_bound,
            threshold: point.response,
        });
    }
    let diff = limits
        .checkpoint_difference(expected, compared, "")
        .ok_or("the shot has no over-bound checkpoint")?;
    let terminal = if [
        "rest_position",
        "rest_surface",
        "end_reason",
        "embedded",
        "playback.rest_position",
        "playback.rest_surface",
        "playback.end_reason",
        "playback.embedded",
        "outcome.rest",
        "outcome.rest_surface",
        "outcome.end",
        "outcome.total_m",
    ]
    .iter()
    .any(|path| diff == *path || diff.starts_with(&format!("{path}.")))
    {
        ordinary
            .iter()
            .rev()
            .find_map(|point| {
                point
                    .path
                    .starts_with("events[")
                    .then(|| point.a.as_dict().or_else(|| point.b.as_dict()))
                    .flatten()
            })
            .and_then(|event| time(event).zip(code(event, "kind").and_then(response)))
    } else {
        None
    };
    Ok(Evidence {
        first,
        rounding,
        over_bound: diff,
        threshold: terminal.map(|(_, kind)| kind),
    })
}

fn include_launch_readouts(points: &mut [Checkpoint], expected: &Dict, actual: &Dict) {
    let launch = points
        .iter_mut()
        .find(|point| point.path == "launch")
        .unwrap();
    launch.a = expected.get("launch").cloned().unwrap_or(Variant::Nil);
    launch.b = actual.get("launch").cloned().unwrap_or(Variant::Nil);
}

fn field_at<'a>(output: &'a Variant, section: &str, key: &str) -> Option<&'a Variant> {
    let output = output.as_dict()?;
    let fields = if section.is_empty() {
        output
    } else {
        output.get(section)?.as_dict()?
    };
    fields.get(key)
}

fn playback(value: &Variant) -> Option<&Variant> {
    let fields = value.as_dict()?;
    Some(fields.get("playback").unwrap_or(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use golf_sim_base::{
        digest::object,
        variant::{Array, Components, FloatWidth},
    };

    fn shot(extra_tick: bool, first_height: Real) -> Variant {
        let period = t::SAMPLE_INTERVAL_S;
        let dt = t::GROUND_DT;
        let phase = if extra_tick { 2e-5 } else { -2e-5 };
        let pure_roll = 2.0 * period + phase;
        let tick = 4.0 * period;
        let rest = tick + dt + phase;
        let preceding = tick - period + phase + if extra_tick { 0.0 } else { dt };
        let surface = Surface::Fairway as i64;
        let point = || {
            Variant::Vector3(Components {
                values: [0.0; 3],
                width: FloatWidth::Double,
            })
        };
        let event = |kind, time_s: Real| {
            object([
                ("kind", (kind as i64).into()),
                ("time_s", time_s.into()),
                ("position", point()),
                ("velocity", point()),
                ("spin", point()),
                ("zone", 0_i64.into()),
                ("surface", surface.into()),
            ])
        };
        let mut times = vec![0.0, period, pure_roll, preceding];
        if extra_tick {
            times.push(tick + phase);
        }
        times.push(rest);
        let mut positions = vec![[0.0; 3]; times.len()];
        positions[1][1] = first_height;
        object([
            ("launch", event(EventKind::Impact, 0.0)),
            (
                "events",
                Variant::Array(Array::new(vec![
                    event(EventKind::Impact, 0.0),
                    event(EventKind::PureRoll, pure_roll),
                    event(EventKind::Rest, rest),
                ])),
            ),
            (
                "touchdowns",
                Variant::Array(Array::new(vec![object([("time_s", (0.025).into())])])),
            ),
            ("times", Variant::PackedFloat64Array(times)),
            (
                "positions",
                Variant::PackedVector3Array(Components {
                    values: positions,
                    width: FloatWidth::Double,
                }),
            ),
            ("rest_surface", surface.into()),
        ])
    }

    #[test]
    fn extra_sample_requires_both_small_early_drift_and_bracketed_rest_tick() {
        let reference = shot(false, 0.0);
        let core = shot(true, STEP);
        let (_, aligned, tick) = align_tick(&reference, &core, 1.0).unwrap();
        assert_eq!(
            Playback::read(&aligned).unwrap().times.len(),
            Playback::read(&reference).unwrap().times.len()
        );
        assert_eq!(
            Playback::read(&aligned).unwrap().times.last(),
            Playback::read(&core).unwrap().times.last()
        );
        assert_eq!(tick, 4.0 * t::SAMPLE_INTERVAL_S);
        let expected = checkpoints(reference.as_dict().unwrap(), core.as_dict().unwrap());
        let first = expected
            .iter()
            .find(|point| point.first_difference().is_some())
            .unwrap();
        assert!(first.rounding_difference().is_none());
        let wrong = shot(true, 5.0 * STEP);
        let expected = checkpoints(reference.as_dict().unwrap(), wrong.as_dict().unwrap());
        let first = expected
            .iter()
            .find(|point| point.first_difference().is_some())
            .unwrap();
        assert!(first.rounding_difference().is_some());
        assert!(align_tick(&reference, &core, 0.5).is_err());
    }

    #[test]
    fn a_free_flight_failure_stays_free_flight_before_a_later_stop() {
        let reference = shot(false, 0.0);
        let core = shot(true, 0.1);
        let limits =
            crate::tolerance::Layer::for_fixture(std::path::Path::new(""), "physics/strokes")
                .unwrap();
        let proof = evidence(&reference, &core, &core, &limits, Some(Ok(())), 1.0).unwrap();
        assert!(proof.over_bound.starts_with("positions[1]"));
        assert_eq!(proof.threshold, None);
    }
}
