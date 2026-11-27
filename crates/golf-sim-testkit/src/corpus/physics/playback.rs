//! Evidence for a periodic sample immediately before the rest event.

use golf_sim_base::{
    Real, Surface, fm,
    variant::{Dict, Variant},
};
use golf_sim_physics::{tunables as t, values::EventKind};

const STEP: Real = f32::EPSILON as Real;
const CLOCK_EPSILON: Real = 1e-9;
const TIME_BOUND: Real = 1e-6;

fn vector(fields: &Dict, key: &str) -> Option<[Real; 3]> {
    let Variant::Vector3(value) = fields.get(key)? else {
        return None;
    };
    Some(value.values)
}

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
    positions: &'a [[Real; 3]],
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
            positions: &positions.values,
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

    fn first_flight_sample_difference(&self, other: &Self) -> Result<String, String> {
        let same_state = |a: &Dict, b: &Dict| {
            ["position", "velocity", "spin"].into_iter().all(|key| {
                vector(a, key)
                    .zip(vector(b, key))
                    .is_some_and(|(a, b)| a == b)
            })
        };
        let same_origin = || {
            let launch = self.shot.get("launch")?.as_dict()?;
            let other_launch = other.shot.get("launch")?.as_dict()?;
            let impact = self.events.first()?.as_dict()?;
            let other_impact = other.events.first()?.as_dict()?;
            Some(
                same_state(launch, other_launch)
                    && same_state(impact, other_impact)
                    && code(impact, "kind")? == EventKind::Impact as i64
                    && code(impact, "kind")? == code(other_impact, "kind")?
                    && code(impact, "zone")? == code(other_impact, "zone")?
                    && code(impact, "surface")? == code(other_impact, "surface")?
                    && time(impact)? == 0.0
                    && time(other_impact)? == 0.0,
            )
        };
        if same_origin() != Some(true) {
            return Err("launch or impact differs before the first flight sample".into());
        }
        for (index, ((&a_time, &b_time), (a, b))) in self
            .times
            .iter()
            .zip(other.times)
            .zip(self.positions.iter().zip(other.positions))
            .enumerate()
        {
            if !a_time.is_finite()
                || !b_time.is_finite()
                || a.iter().chain(b).any(|value| !value.is_finite())
            {
                return Err("non-finite flight sample".into());
            }
            if a_time == b_time && a == b {
                continue;
            }
            let cutoff = a_time.max(b_time);
            let clear = |playback: &Self| {
                playback
                    .events
                    .get(1)
                    .and_then(Variant::as_dict)
                    .and_then(time)
                    .is_some_and(|stamp| stamp > cutoff)
                    && array(playback.shot, "touchdowns")
                        .and_then(|touches| touches.first())
                        .and_then(Variant::as_dict)
                        .and_then(time)
                        .is_some_and(|stamp| stamp > cutoff)
            };
            if !clear(self) || !clear(other) {
                return Err("an event precedes the first unequal flight sample".into());
            }
            let time_error = (a_time - b_time).abs();
            let position_error = a
                .iter()
                .zip(b)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, Real::max);
            let scale = a.iter().chain(b).map(|n| n.abs()).fold(1.0, Real::max);
            let bound = 4.0 * STEP * scale;
            let detail = format!(
                "sample {index}: time difference {time_error:.6e} s (bound {TIME_BOUND:.1e} s), position difference {position_error:.6e} m (bound {bound:.6e} m)"
            );
            return if time_error <= TIME_BOUND && position_error <= bound {
                Ok(detail)
            } else {
                Err(format!("first difference exceeds rounding: {detail}"))
            };
        }
        Err("no differing flight sample".into())
    }
}

pub(crate) struct Alignment {
    pub expected: Variant,
    pub actual: Variant,
    pub first: String,
    pub tick: Real,
}

pub(crate) fn align_stop_tick(
    expected: &Variant,
    actual: &Variant,
    step_scale: Real,
) -> Result<Alignment, String> {
    let reference = Playback::read(expected).ok_or("reference playback is incomplete")?;
    let computed = Playback::read(actual).ok_or("core playback is incomplete")?;
    let first = reference.first_flight_sample_difference(&computed)?;
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
    Ok(Alignment {
        expected,
        actual,
        first,
        tick,
    })
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
        let aligned = align_stop_tick(&reference, &core, 1.0).unwrap();
        assert!(aligned.first.starts_with("sample 1:"));
        assert_eq!(
            Playback::read(&aligned.actual).unwrap().times.len(),
            Playback::read(&reference).unwrap().times.len()
        );
        assert_eq!(
            Playback::read(&aligned.actual).unwrap().times.last(),
            Playback::read(&core).unwrap().times.last()
        );
        assert_eq!(aligned.tick, 4.0 * t::SAMPLE_INTERVAL_S);
        assert!(
            align_stop_tick(&reference, &shot(true, 5.0 * STEP), 1.0)
                .err()
                .unwrap()
                .starts_with("first difference exceeds rounding")
        );
        assert!(align_stop_tick(&reference, &core, 0.5).is_err());
    }
}
