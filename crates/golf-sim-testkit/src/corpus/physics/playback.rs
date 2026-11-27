//! Compare chronological shot checkpoints and verify response thresholds.

use golf_sim_base::{
    Real, Surface, Vec3, fm,
    variant::{Dict, Variant},
};
use golf_sim_physics::{
    integrator::Checkpoint as IntegrationCheckpoint, tunables as t, values::EventKind,
};

use crate::tolerance::Layer;

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

    fn replay_failure(&self, prefix: &str, field: &str, shot: &Dict, reason: &str) -> String {
        let phase = if let Some(response) = self.response {
            response
        } else if self.time > 0.0 && in_flight(shot, self.time) {
            "free flight"
        } else {
            "away from a response"
        };
        format!(
            "{prefix}{} at {:.9} s ({phase}; {reason})",
            self.field(field),
            self.time
        )
    }
}

fn in_flight(shot: &Dict, stamp: Real) -> bool {
    array(shot, "events")
        .unwrap_or_default()
        .iter()
        .filter_map(|event| {
            let event = event.as_dict()?;
            let time = time(event)?;
            let kind = code(event, "kind")?;
            let flying = match kind {
                k if [EventKind::Impact, EventKind::Release, EventKind::Bounce]
                    .iter()
                    .any(|event| *event as i64 == k) =>
                {
                    true
                }
                k if [
                    EventKind::Landing,
                    EventKind::RollStart,
                    EventKind::PureRoll,
                    EventKind::CupEntry,
                    EventKind::Holed,
                    EventKind::Rest,
                ]
                .iter()
                .any(|event| *event as i64 == k) =>
                {
                    false
                }
                _ => return None,
            };
            (time <= stamp).then_some((time, flying))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .is_some_and(|(_, flying)| flying)
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
        let is_apex = [&checkpoint.a, &checkpoint.b].iter().any(|event| {
            event.as_dict().and_then(|fields| code(fields, "kind")) == Some(EventKind::Apex as i64)
        });
        if !is_apex {
            checkpoint.response = checkpoint
                .response
                .or_else(|| history(checkpoint.time, t::FLIGHT_DT));
        }
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

pub(crate) fn check_initial_rounding(expected: &Variant, raw: &Variant) -> Result<String, String> {
    let points = checkpoints(shot_fields(expected)?, shot_fields(raw)?);
    let Some((first, detail)) = points
        .iter()
        .find_map(|point| Some((point, point.first_difference()?)))
    else {
        return Err("no differing trajectory checkpoint".into());
    };
    for point in points.iter().take_while(|point| point.time <= first.time) {
        if let Some(field) = point.rounding_difference() {
            return Err(format!(
                "first checkpoint exceeds float rounding at {}",
                point.field(&field)
            ));
        }
    }
    Ok(detail)
}

#[derive(Debug)]
pub(crate) struct VerifiedResponse {
    pub threshold: &'static str,
    pub first_over_bound: String,
}

pub(crate) fn verify_response(
    expected: &Variant,
    computed: &Variant,
    limits: &Layer,
    step_scale: Real,
    replay: impl FnOnce(IntegrationCheckpoint) -> Result<Variant, String>,
) -> Result<Option<VerifiedResponse>, String> {
    let reference = shot_fields(expected)?;
    let actual = shot_fields(computed)?;
    let prefix = if expected
        .as_dict()
        .is_some_and(|fields| fields.get("playback").is_some())
    {
        "playback."
    } else {
        ""
    };
    let aligned = align_outputs(expected, computed, limits, prefix, step_scale);
    let (a, b) = aligned
        .as_ref()
        .map_or((expected, computed), |(a, b)| (a, b));
    let Some(failure) = first_failure(a, b, prefix, limits) else {
        return Ok(aligned.as_ref().and_then(|_| {
            Some(VerifiedResponse {
                threshold: "stop playback tick",
                first_over_bound: first_failure(expected, computed, prefix, limits)?.message,
            })
        }));
    };
    if let Some(field) = limits.checkpoint_discrete_difference(a, b) {
        return Err(format!(
            "{}; discrete difference at {field}",
            failure.message
        ));
    }
    let Some(threshold) = failure.response else {
        return Err(failure.message);
    };
    let Some(checkpoint) = incoming_checkpoint(reference, actual, prefix, limits, &failure) else {
        return Err(failure.message);
    };
    let resumed = replay(checkpoint).map_err(|error| {
        format!(
            "{}; restarting at {:?}: {error}",
            failure.message, checkpoint.kind
        )
    })?;
    let aligned = align_outputs(expected, &resumed, limits, prefix, step_scale);
    let (a, b) = aligned
        .as_ref()
        .map_or((expected, &resumed), |(a, b)| (a, b));
    if let Some(after) = first_failure(a, b, prefix, limits) {
        return Err(if failure.message == after.message {
            failure.message
        } else {
            format!(
                "{}; restarting at {:?}: {}",
                failure.message, checkpoint.kind, after.message
            )
        });
    }
    Ok(Some(VerifiedResponse {
        threshold,
        first_over_bound: failure.message,
    }))
}

pub(crate) fn first_response_difference(
    expected: &Variant,
    computed: &Variant,
    limits: &Layer,
) -> Option<(&'static str, Real)> {
    let failure = first_failure(expected, computed, "playback.", limits)?;
    Some((failure.response?, failure.time))
}

struct Failure {
    message: String,
    time: Real,
    path: String,
    response: Option<&'static str>,
}

fn first_failure(
    expected: &Variant,
    computed: &Variant,
    prefix: &str,
    limits: &Layer,
) -> Option<Failure> {
    let field = limits
        .checkpoint_difference(expected, computed, "")
        .or_else(|| limits.checkpoint_discrete_difference(expected, computed))?;
    let reference = shot_fields(expected).ok()?;
    let actual = shot_fields(computed).ok()?;
    let mut points = checkpoints(reference, actual);
    include_launch_readouts(&mut points, reference, actual);
    for point in &points {
        let path = format!("{prefix}{}", point.path);
        if let Some(field) = limits.checkpoint_difference(&point.a, &point.b, &path) {
            return Some(Failure {
                message: point.replay_failure(prefix, &field, reference, "exceeds output row"),
                time: point.time,
                path: point.path.clone(),
                response: point.response,
            });
        }
    }
    let terminal = ["rest_position", "outcome.rest", "outcome.total_m"]
        .into_iter()
        .any(|key| {
            let bare = field.strip_prefix(prefix).unwrap_or(&field);
            bare == key
                || bare
                    .strip_prefix(key)
                    .is_some_and(|tail| tail.starts_with('.'))
        });
    let time = points.last().map_or(0.0, |point| point.time);
    Some(Failure {
        message: format!("{field} at {time:.9} s (whole-shot output)"),
        time,
        path: field,
        response: terminal
            .then(|| {
                let last = array(reference, "events")?.last()?.as_dict()?;
                response(code(last, "kind")?)
            })
            .flatten(),
    })
}

fn incoming_checkpoint(
    reference: &Dict,
    actual: &Dict,
    prefix: &str,
    limits: &Layer,
    failure: &Failure,
) -> Option<IntegrationCheckpoint> {
    let reference = array(reference, "events")?;
    let actual = array(actual, "events")?;
    let vector = |fields: &Dict, key| {
        let Variant::Vector3(point) = fields.get(key)? else {
            return None;
        };
        let [x, y, z] = point.values;
        Some(Vec3::new(x, y, z))
    };
    reference
        .iter()
        .zip(actual)
        .enumerate()
        .rev()
        .find_map(|(event_index, (a, b))| {
            let fields = a.as_dict()?;
            let time_s = time(fields)?;
            let path = format!("events[{event_index}]");
            let kind = [
                EventKind::Landing,
                EventKind::Bounce,
                EventKind::RollStart,
                EventKind::PureRoll,
                EventKind::Obstacle,
                EventKind::CupEntry,
                EventKind::LipOut,
            ]
            .into_iter()
            .find(|kind| code(fields, "kind") == Some(*kind as i64))?;
            if time_s > failure.time
                || path == failure.path
                || (time_s == failure.time && failure.path.starts_with("touchdowns["))
                || limits
                    .checkpoint_difference(a, b, &format!("{prefix}{path}"))
                    .is_some()
            {
                return None;
            }
            Some(IntegrationCheckpoint {
                event_index,
                kind,
                time_s,
                position: vector(fields, "position")?,
                velocity: vector(fields, "velocity")?,
                spin: vector(fields, "spin")?,
            })
        })
}

fn align_outputs(
    expected: &Variant,
    computed: &Variant,
    limits: &Layer,
    prefix: &str,
    step_scale: Real,
) -> Option<(Variant, Variant)> {
    let (a, b, _) = align_tick(playback(expected)?, playback(computed)?, step_scale).ok()?;
    let (a_shot, b_shot) = (a.as_dict()?, b.as_dict()?);
    let a_before = shot_fields(expected).ok()?;
    let b_before = shot_fields(computed).ok()?;
    let positions = |dict: &Dict| match dict.get("positions") {
        Some(Variant::PackedVector3Array(points)) => Some(points.values.clone()),
        _ => None,
    };
    let (long, short) = if positions(a_before)?.len() > positions(b_before)?.len() {
        (positions(a_before)?, positions(b_shot)?)
    } else {
        (positions(b_before)?, positions(a_shot)?)
    };
    let index = long.len() - 2;
    let midpoint = std::array::from_fn(|axis| (short[index - 1][axis] + short[index][axis]) * 0.5);
    limits
        .checkpoint_difference(
            &vector_variant(long[index]),
            &vector_variant(midpoint),
            &format!("{prefix}positions[{index}]"),
        )
        .is_none()
        .then(|| {
            let with_playback = |result: &Variant, shot: Variant| {
                if prefix.is_empty() {
                    return shot;
                }
                let mut output = result.clone();
                if let Variant::Dict(fields) = &mut output {
                    *fields.get_mut("playback").unwrap() = shot;
                }
                output
            };
            (with_playback(expected, a), with_playback(computed, b))
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

pub(crate) fn playback(value: &Variant) -> Option<&Variant> {
    let fields = value.as_dict()?;
    Some(fields.get("playback").unwrap_or(value))
}

fn shot_fields(value: &Variant) -> Result<&Dict, &'static str> {
    playback(value)
        .and_then(Variant::as_dict)
        .ok_or("missing shot checkpoints")
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
    fn extra_tick_position_still_has_a_row() {
        let reference = shot(false, 0.0);
        let core = shot(true, STEP);
        let limits = Layer::for_fixture(std::path::Path::new(""), "physics/strokes").unwrap();
        let replay = |_| panic!("the aligned trajectory should be checked directly");
        let proof = verify_response(&reference, &core, &limits, 1.0, replay)
            .unwrap()
            .unwrap();
        assert_eq!(proof.threshold, "stop playback tick");
        let mut core = core;
        let Variant::Dict(fields) = &mut core else {
            unreachable!()
        };
        let Some(Variant::PackedVector3Array(positions)) = fields.get_mut("positions") else {
            unreachable!()
        };
        let extra = positions.values.len() - 2;
        positions.values[extra][0] = 100.0;
        assert!(verify_response(&reference, &core, &limits, 1.0, replay).is_err());
    }

    #[test]
    fn stop_replay_checks_rest_and_end_reason() {
        use golf_sim_base::trace::NoProbe;
        use golf_sim_physics::{
            integrator::{self, Options},
            plane::{ConstantAir, PlaneGround},
            values::Launch,
        };

        let ground = PlaneGround::default();
        let air = ConstantAir::new(golf_sim_physics::facade::standard_air());
        let radius = t::BALL.radius_m;
        let speed = t::ROLL_STOP_SPEED - 0.01 * STEP;
        let launch = Launch {
            position: Vec3::new(0.0, radius, 0.0),
            velocity: Vec3::new(speed, 0.0, 0.0),
            spin: Vec3::new(0.0, 0.0, -speed / radius),
            ..Launch::default()
        };
        let options = Options::default();
        let computed = integrator::run(
            &launch,
            &t::BALL,
            &air,
            &ground,
            None,
            options,
            &mut NoProbe,
        );
        let (event_index, roll) = computed
            .events
            .iter()
            .enumerate()
            .find(|(_, event)| event.kind == EventKind::PureRoll)
            .unwrap();
        let replay = |state| {
            let shot = integrator::run_with_checkpoint(
                &launch,
                &t::BALL,
                &air,
                &ground,
                None,
                options,
                state,
            )
            .ok_or("missing roll checkpoint")?;
            Ok(super::super::shot_snapshot(&shot))
        };
        let speed = t::ROLL_STOP_SPEED + 0.01 * STEP;
        let state = IntegrationCheckpoint {
            event_index,
            kind: EventKind::PureRoll,
            time_s: roll.time_s,
            position: roll.position,
            velocity: Vec3::new(speed, 0.0, 0.0),
            spin: Vec3::new(0.0, 0.0, -speed / radius),
        };
        let expected = replay(state).unwrap();
        let computed = super::super::shot_snapshot(&computed);
        let limits = Layer::for_fixture(std::path::Path::new(""), "physics/strokes").unwrap();
        assert!(check_initial_rounding(&expected, &computed).is_ok());
        let proof = verify_response(&expected, &computed, &limits, 1.0, replay)
            .unwrap()
            .unwrap();
        assert_eq!(proof.threshold, "stop");

        for (key, wrong) in [
            ("rest_position", vector_variant([100.0, radius, 0.0])),
            ("end_reason", 99_i64.into()),
        ] {
            let mut mutated = expected.clone();
            let Variant::Dict(fields) = &mut mutated else {
                unreachable!()
            };
            *fields.get_mut(key).unwrap() = wrong;
            let error = verify_response(&mutated, &computed, &limits, 1.0, replay).unwrap_err();
            assert!(error.contains(key), "{error}");
        }
    }

    #[test]
    fn a_free_flight_failure_stays_free_flight_before_a_later_stop() {
        let reference = shot(false, 0.0);
        let core = shot(true, 0.1);
        let limits =
            crate::tolerance::Layer::for_fixture(std::path::Path::new(""), "physics/strokes")
                .unwrap();
        let failure = verify_response(&reference, &core, &limits, 1.0, |_| {
            panic!("a free flight failure cannot use a later response")
        })
        .unwrap_err();
        assert!(failure.starts_with("positions[1].y"));
        assert!(failure.contains("free flight"));
    }
}
