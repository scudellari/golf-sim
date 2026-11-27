use golf_sim_base::{
    ClubId, Real, Vec3,
    digest::object,
    trace::NoProbe,
    variant::{Dict, Variant},
};
use golf_sim_conditions::world::World;
use golf_sim_physics::{
    facade as sim,
    plane::{ConstantAir, PlaneGround},
    samplers::{AirSampler, GroundSample, GroundSampler},
    values::{EventKind, Launch, ShotRequest, ShotResult},
};
use golf_sim_stroke::{
    address::Address,
    stroke::{self, StrokeInput},
};

use crate::{
    corpus::physics::{self, flight::carry_rounding_bound, impact, playback},
    tolerance::Layer,
};

use super::facade::{
    DerivedChecks, dict, field, fields_mut, horizontal, impact_request, launch_snapshot, magnitude,
    number, vector, vector3,
};

const STEP: Real = f32::EPSILON as Real;

struct ShotAt<'a> {
    request: &'a ShotRequest,
    air: &'a dyn AirSampler,
    ground: &'a dyn GroundSampler,
    playback: bool,
}

impl ShotAt<'_> {
    fn strike(&self) -> Launch {
        sim::strike(self.request, self.air, self.ground, &mut NoProbe)
    }

    fn fly(&self, launch: &Launch) -> ShotResult {
        sim::simulate_launch(
            launch,
            &self.request.ball,
            self.request.time_s,
            self.air,
            self.ground,
            self.request.cup,
            self.playback,
            &mut NoProbe,
        )
    }

    fn carry(&self, launch: &Launch, level: Real) -> Real {
        sim::carry_to_height(
            launch,
            &self.request.ball,
            self.request.time_s,
            self.air,
            self.ground,
            level,
        )
    }

    fn check_carry(&self, launch: &Launch, level: Real, values: [Real; 2], limits: &Layer) -> bool {
        let [reference, actual] = values;
        same_number(reference, actual)
            || limits
                .checkpoint_difference(&reference.into(), &actual.into(), "outcome.carry_m")
                .is_none()
            || (reference - actual).abs()
                <= carry_rounding_bound(
                    launch,
                    &self.request.ball,
                    self.request.time_s,
                    self.air,
                    self.ground,
                    level,
                    actual,
                )
    }

    fn check_launch(
        &self,
        case: &Dict,
        expected: &Variant,
        actual: &Variant,
        limits: &Layer,
    ) -> Result<(), String> {
        if expected == &Variant::Nil && actual == &Variant::Nil {
            return Ok(());
        }
        let reference = expected.as_dict().ok_or("missing recorded launch")?;
        let mut computed = actual.as_dict().ok_or("missing computed launch")?.clone();
        impact::compare_launch_rounding(
            case,
            reference,
            &mut computed,
            self.request,
            self.ground,
            false,
        )?;
        let mut checks = DerivedChecks {
            id: field(case, "id").as_str().unwrap(),
            drift: false,
            failure: None,
        };
        checks.launch(reference, &mut computed, true);
        if let Some(failure) = checks.failure {
            return Err(failure);
        }
        if let Some(path) =
            limits.checkpoint_difference(expected, &Variant::Dict(computed.clone()), "")
        {
            let detail = match (reference.get(&path), computed.get(&path)) {
                (Some(Variant::Float(a)), Some(Variant::Float(b))) => {
                    format!(" (recorded {:.6e}, computed {:.6e})", a.value(), b.value())
                }
                _ => String::new(),
            };
            return Err(format!("launch {path} exceeds its row{detail}"));
        }
        Ok(())
    }

    fn threshold(
        &self,
        reference: &Variant,
        actual: &ShotResult,
        limits: &Layer,
        matches_record: impl FnOnce(&ShotResult, Real) -> bool,
    ) -> Option<&'static str> {
        let reference = reference.as_dict()?;
        let replay = self.fly(&physics::recorded_launch(reference));
        let recorded = physics::shot_snapshot(&replay);
        let computed = physics::shot_snapshot(actual);
        let launch = launch_snapshot(actual.launch?);
        impact::verify_strike_rounding(reference, dict(&launch), self.request, self.ground).ok()?;
        let mut compared = computed;
        impact::compare_shot_launch(
            dict(&recorded),
            fields_mut(&mut compared),
            |expected, actual| {
                *actual = expected.clone();
                Ok(())
            },
        )
        .ok()?;
        let wrap = |shot| object([("playback", shot)]);
        let (threshold, time) =
            playback::first_response_difference(&wrap(recorded), &wrap(compared), limits)?;
        matches_record(&replay, time).then_some(threshold)
    }
}

fn same_number(a: Real, b: Real) -> bool {
    a == b || a.is_nan() && b.is_nan()
}

// Check each side's formula, then propagate the observed operand changes through it.
fn formula(fields: [Real; 2], rebuilt: [Real; 2], reference_rounding: Real) -> Result<(), String> {
    let [expected, actual] = fields;
    let [reference, computed] = rebuilt;
    let within = |a: Real, b: Real, bound: Real| {
        same_number(a, b) || a.is_finite() && b.is_finite() && (a - b).abs() <= bound
    };
    let core_rounding = 4.0 * Real::EPSILON * computed.abs().max(1.0);
    if !within(actual, computed, core_rounding) {
        return Err(format!(
            "computed {actual:.6e} disagrees with its operands ({computed:.6e})"
        ));
    }
    if !within(expected, reference, reference_rounding) {
        return Err(format!(
            "recorded {expected:.6e} disagrees with its operands ({reference:.6e})"
        ));
    }
    let propagated = (reference - computed).abs();
    if !within(
        expected,
        actual,
        propagated + reference_rounding + core_rounding,
    ) {
        return Err(format!(
            "delta {:.6e} exceeds propagated operand bound {:.6e}",
            (expected - actual).abs(),
            propagated + reference_rounding + core_rounding
        ));
    }
    Ok(())
}

struct PreviewChecks<'a> {
    case: &'a Dict,
    reference: &'a Dict,
    actual: &'a mut Variant,
    limits: &'a Layer,
    failures: Vec<String>,
    recorded_operands: &'a Dict,
    operands: Vec<(&'static str, Variant)>,
}

impl PreviewChecks<'_> {
    fn check(&mut self, field: &str, result: Result<(), String>) -> bool {
        match result {
            Ok(()) => true,
            Err(reason) => {
                self.failures.push(format!("{field}: {reason}"));
                false
            }
        }
    }

    fn formula(&mut self, key: &str, rebuilt: [Real; 2], rounding: Real, operands_pass: bool) {
        if !operands_pass {
            self.check(key, Err("operand exceeds its row".into()));
            return;
        }
        let actual = fields_mut(self.actual);
        let values = [number(self.reference, key), number(actual, key)];
        let result = formula(values, rebuilt, rounding);
        if result.is_ok() {
            *actual.get_mut(key).unwrap() = field(self.reference, key).clone();
        }
        self.check(key, result);
    }

    fn launch(
        &mut self,
        key: &str,
        shot: &ShotAt<'_>,
        reference: &Variant,
        actual: &Variant,
    ) -> bool {
        self.check(
            key,
            shot.check_launch(self.case, reference, actual, self.limits),
        )
    }

    fn carry(
        &mut self,
        key: &str,
        shot: &ShotAt<'_>,
        launch: &Launch,
        height: Real,
        values: [Real; 2],
    ) -> bool {
        self.check(
            key,
            if shot.check_carry(launch, height, values, self.limits) {
                Ok(())
            } else {
                Err(format!(
                    "recorded {:.6e}, computed {:.6e}",
                    values[0], values[1]
                ))
            },
        )
    }

    fn roll(&mut self, shot: &ShotAt<'_>, launch: &Launch) {
        let roll = shot.fly(launch);
        let Some(expected) = self
            .recorded_operands
            .get("roll")
            .and_then(Variant::as_dict)
        else {
            self.check("roll_m", Err("missing recorded roll operand".into()));
            return;
        };
        let actual_launch = roll.launch.map_or(Variant::Nil, launch_snapshot);
        let reference_launch = field(expected, "launch");
        let launch_ok = self.launch(
            "operands.roll.launch",
            shot,
            reference_launch,
            &actual_launch,
        );
        let reference_rest = field(expected, "rest_position");
        let rest_matches = |roll: &ShotResult| {
            self.limits
                .checkpoint_difference(
                    reference_rest,
                    &vector(roll.rest_position),
                    "playback.rest_position",
                )
                .is_none()
        };
        let within_row = rest_matches(&roll);
        let threshold = if launch_ok && !within_row {
            shot.threshold(reference_launch, &roll, self.limits, |replay, _| {
                rest_matches(replay)
            })
        } else {
            None
        };
        let rest_ok = self.check(
            "operands.roll.rest_position",
            if within_row || threshold.is_some() {
                Ok(())
            } else {
                Err("rest exceeds its stroke row".into())
            },
        );
        if let Some(threshold) = threshold {
            self.departed_operand("roll", threshold);
        }
        let b = vector3(reference_rest);
        let origin = reference_launch
            .as_dict()
            .map(|launch| vector3(field(launch, "position")));
        let reference_roll = origin.map_or(0.0, |a| horizontal(b - a).length());
        let computed_roll = roll.launch.map_or(0.0, |launch| {
            horizontal(roll.rest_position - launch.position).length()
        });
        let scale = origin.map_or(1.0_f64, magnitude).max(magnitude(b)).max(1.0);
        self.formula(
            "roll_m",
            [reference_roll, computed_roll],
            4.0 * STEP * scale,
            launch_ok && rest_ok,
        );

        self.operands.push((
            "roll",
            object([
                ("launch", actual_launch),
                ("rest_position", vector(roll.rest_position)),
            ]),
        ));
    }

    fn departed_operand(&self, name: &str, threshold: &str) {
        let id = field(self.case, "id").as_str().unwrap();
        println!("case {id}: preview operand {name} departs at {threshold}");
    }

    fn nominal(
        &mut self,
        name: &str,
        input: &StrokeInput,
        expected: &Dict,
        standard: &ConstantAir,
        level: &PlaneGround,
    ) -> Hint {
        let request = impact_request(input, stroke::nominal_execution(input), false);
        let shot = ShotAt {
            request: &request,
            air: standard,
            ground: level,
            playback: false,
        };
        let flight = sim::simulate_shot(&request, standard, level, false, &mut NoProbe);
        let reference_launch = field(expected, "launch");
        let actual_launch = flight.launch.map_or(Variant::Nil, launch_snapshot);
        let launch_ok = self.launch(
            &format!("operands.{name}.launch"),
            &shot,
            reference_launch,
            &actual_launch,
        );
        let metrics = [
            (
                "carry_m",
                "outcome.carry_m",
                ShotResult::carry_m as fn(&ShotResult) -> Real,
            ),
            (
                "apex_height_m",
                "playback.events[0].position",
                ShotResult::apex_height_m,
            ),
            (
                "total_side_m",
                "playback.rest_position",
                ShotResult::total_side_m,
            ),
        ];
        let in_row = |key: &str, row: &str, result: Real| {
            self.limits
                .checkpoint_difference(field(expected, key), &result.into(), row)
                .is_none()
        };
        let passed = metrics.map(|(key, row, derive)| in_row(key, row, derive(&flight)));
        let threshold = if launch_ok && passed.contains(&false) {
            shot.threshold(
                reference_launch,
                &flight,
                self.limits,
                |replay, response_time| {
                    let metric_times = |shot: &ShotResult| {
                        let end = shot.events.last().map_or(0.0, |event| event.time_s);
                        [
                            shot.touchdowns
                                .first()
                                .map_or(end, |contact| contact.time_s),
                            shot.first_event(EventKind::Apex)
                                .map_or(end, |apex| apex.time_s),
                            end,
                        ]
                    };
                    let recorded_times = metric_times(replay);
                    let actual_times = metric_times(&flight);
                    // The reference launch must reproduce the recorded summaries. A response
                    // can account for a summary only if that summary includes the response.
                    metrics
                        .iter()
                        .enumerate()
                        .all(|(index, (key, row, derive))| {
                            in_row(key, row, derive(replay))
                                && (passed[index]
                                    || response_time
                                        <= recorded_times[index].min(actual_times[index]))
                        })
                },
            )
        } else {
            None
        };
        if let Some(threshold) = threshold {
            self.departed_operand(name, threshold);
        }
        let operands = metrics.map(|(key, _, derive)| [number(expected, key), derive(&flight)]);
        let passed = std::array::from_fn(|index| {
            let (key, _, _) = metrics[index];
            let [a, b] = operands[index];
            self.check(
                &format!("operands.{name}.{key}"),
                if passed[index] || threshold.is_some() {
                    Ok(())
                } else {
                    Err(format!("recorded {a:.6e}, computed {b:.6e}"))
                },
            ) && launch_ok
        });
        let mut snapshot = vec![("launch", actual_launch)];
        snapshot.extend(
            metrics
                .into_iter()
                .zip(operands)
                .map(|((key, _, _), values)| (key, values[1].into())),
        );
        Hint {
            operands,
            passed,
            snapshot: object(snapshot),
        }
    }

    fn carry_and_roll(
        &mut self,
        here: &ShotAt<'_>,
        launch: &Launch,
        putt: bool,
        launch_ok: bool,
    ) -> bool {
        let height = here.request.position.y;
        let mut carry = [Real::NAN; 2];
        let carry_ok = if putt {
            self.recorded_operands.get("carry_m").is_none()
        } else {
            carry[0] = self
                .recorded_operands
                .get("carry_m")
                .map_or(Real::NAN, |_| number(self.recorded_operands, "carry_m"));
            carry[1] = here.carry(launch, height);
            self.operands.push(("carry_m", carry[1].into()));
            self.carry("operands.carry_m", here, launch, height, carry)
        };
        self.formula(
            "carry_m",
            carry,
            STEP * carry[0].abs().max(1.0),
            launch_ok && carry_ok,
        );
        if carry[1].is_nan() {
            self.roll(here, launch);
            false
        } else {
            self.formula(
                "roll_m",
                [Real::NAN; 2],
                0.0,
                self.recorded_operands.get("roll").is_none(),
            );
            true
        }
    }

    fn target(
        &mut self,
        target: Option<Vec3>,
        here: &ShotAt<'_>,
        still: &ShotAt<'_>,
        launch: &Launch,
        launch_ok: bool,
    ) {
        let Some(target) = target else {
            for key in ["target_distance_m", "carry_to_target_m", "plays_like_m"] {
                self.formula(key, [Real::NAN; 2], 0.0, true);
            }
            return;
        };
        let start = here.request.position;
        let height = here.ground.sample_ground(target.x, target.z).height_m;
        let standard_launch = still.strike();
        let standard_snapshot = launch_snapshot(standard_launch);
        let operands = self.recorded_operands;
        let (Some(reference_launch), Some(_), Some(_)) = (
            operands.get("standard_launch"),
            operands.get("carry_to_target_m"),
            operands.get("carry_standard_m"),
        ) else {
            self.check(
                "operands",
                Err("missing recorded plays-like operands".into()),
            );
            return;
        };
        let to_target = [
            number(operands, "carry_to_target_m"),
            here.carry(launch, height),
        ];
        let carry_standard = [
            number(operands, "carry_standard_m"),
            still.carry(&standard_launch, start.y),
        ];
        let standard_ok = self.launch(
            "operands.standard_launch",
            still,
            reference_launch,
            &standard_snapshot,
        );
        let to_target_ok = self.carry(
            "operands.carry_to_target_m",
            here,
            launch,
            height,
            to_target,
        );
        let carry_standard_ok = self.carry(
            "operands.carry_standard_m",
            still,
            &standard_launch,
            start.y,
            carry_standard,
        );
        self.operands.extend([
            ("carry_to_target_m", to_target[1].into()),
            ("standard_launch", standard_snapshot),
            ("carry_standard_m", carry_standard[1].into()),
        ]);
        let distance = horizontal(target - start).length();
        let scale = magnitude(start).max(magnitude(target)).max(1.0);
        self.formula("target_distance_m", [distance; 2], 4.0 * STEP * scale, true);
        self.formula(
            "carry_to_target_m",
            to_target,
            STEP * to_target[0].abs().max(1.0),
            launch_ok && to_target_ok,
        );
        let distance = [number(self.reference, "target_distance_m"), distance];
        let plays_like = std::array::from_fn(|i| {
            if to_target[i] > 0.0 && carry_standard[i].is_finite() {
                distance[i] * carry_standard[i] / to_target[i]
            } else {
                Real::NAN
            }
        });
        self.formula(
            "plays_like_m",
            plays_like,
            2.0 * STEP * plays_like[0].abs().max(1.0),
            launch_ok && standard_ok && to_target_ok && carry_standard_ok,
        );
    }

    fn hints(&mut self, input: &StrokeInput, baseline: Address, standard: &ConstantAir) {
        let fields = ["hint_carry_m", "hint_apex_m", "hint_offline_m"];
        if input.delivery.address.same_as(&baseline) {
            for field in fields {
                self.formula(field, [0.0; 2], 0.0, true);
            }
            return;
        }
        let (Some(mine), Some(theirs)) = (
            self.recorded_operands
                .get("mine")
                .and_then(Variant::as_dict),
            self.recorded_operands
                .get("theirs")
                .and_then(Variant::as_dict),
        ) else {
            self.check("operands", Err("missing recorded hint operands".into()));
            return;
        };
        let level = PlaneGround::new(GroundSample::default(), input.ball_position());
        let mut stock = input.clone();
        stock.delivery.address = baseline;
        let mine = self.nominal("mine", input, mine, standard, &level);
        let theirs = self.nominal("theirs", &stock, theirs, standard, &level);
        for (index, key) in fields.into_iter().enumerate() {
            let [a, b] = mine.operands[index];
            let [c, d] = theirs.operands[index];
            let rounding = STEP * a.abs().max(c.abs()).max((a - c).abs()).max(1.0);
            self.formula(
                key,
                [a - c, b - d],
                rounding,
                mine.passed[index] && theirs.passed[index],
            );
        }
        self.operands
            .extend([("mine", mine.snapshot), ("theirs", theirs.snapshot)]);
    }

    fn compare_other_fields(&mut self) {
        for key in [
            "wind_aloft_mps",
            "wind_here_mps",
            "aloft",
            "putt_line",
            "has_hint",
        ] {
            let a = field(self.reference, key);
            let b = field(dict(self.actual), key);
            self.check(
                key,
                self.limits
                    .checkpoint_difference(a, b, key)
                    .map_or(Ok(()), |field| Err(format!("{field} exceeds its row"))),
            );
        }
    }
}

struct Hint {
    operands: [[Real; 2]; 3],
    passed: [bool; 3],
    snapshot: Variant,
}

pub(super) fn compare(
    case: &Dict,
    world: &World<'_>,
    limits: &Layer,
    actual: Variant,
    drift: bool,
) -> Result<Variant, String> {
    let reference = dict(field(case, "output"));
    let expected = dict(field(reference, "operands"));
    let mut projected = actual.clone();
    let mut checks = PreviewChecks {
        case,
        reference,
        actual: &mut projected,
        limits,
        failures: Vec::new(),
        recorded_operands: expected,
        operands: Vec::new(),
    };
    let args = dict(field(case, "input"));
    let input = super::facade::stroke_input(dict(field(args, "input")));
    let target = vector3(field(args, "target"));
    let start = input.ball_position();
    let ground = world.ground();
    let air = world.air_mean(start);
    let standard = ConstantAir::new(sim::standard_air());
    let mut request = impact_request(&input, stroke::nominal_execution(&input), false);
    request.time_s = world.clock_s();
    request.cup = world.cup(input.flagstick_in);
    let putt = input.delivery.club == ClubId::Putter;
    let here = ShotAt {
        request: &request,
        air: &air,
        ground,
        playback: putt,
    };
    let still = ShotAt {
        air: &standard,
        ..here
    };
    let launch = here.strike();
    let launch_variant = launch_snapshot(launch);
    let launch_ok = checks.launch(
        "operands.launch",
        &here,
        field(expected, "launch"),
        &launch_variant,
    );
    checks.operands.push(("launch", launch_variant));
    let airborne = checks.carry_and_roll(&here, &launch, putt, launch_ok);
    checks.target(
        (airborne && target.is_finite()).then_some(target),
        &here,
        &still,
        &launch,
        launch_ok,
    );
    let baseline = field(args, "stock_address")
        .as_dict()
        .map(Address::from_dict)
        .unwrap_or_else(|| stroke::stock(input.delivery.club));
    checks.hints(&input, baseline, &standard);
    checks.compare_other_fields();
    if !checks.failures.is_empty() {
        return Err(checks.failures.join("; "));
    }
    let operands = checks.operands;
    if expected.entries().len() != operands.len()
        || operands
            .iter()
            .any(|(name, _)| expected.get(name).is_none())
    {
        return Err("operands: preview operand shape differs".into());
    }
    let (result, operands) = if drift {
        (actual, object(operands))
    } else {
        (projected, field(reference, "operands").clone())
    };
    let mut entries = dict(&result).entries().to_vec();
    entries.push(("operands".into(), operands));
    Ok(Variant::Dict(Dict::new(entries)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn good_operand_pair_does_not_hide_an_incorrect_hint_formula() {
        let rounding = 100.0 * STEP;
        let mine = [100.0, 100.02];
        let theirs = [99.99, 100.0];
        let diff = [mine[0] - theirs[0], mine[1] - theirs[1]];
        assert!(formula(diff, diff, rounding).is_ok());
        assert!(formula([diff[0], diff[1] + 0.001], diff, rounding).is_err());
        assert!(formula([diff[0] - 0.001, diff[1]], diff, rounding).is_err());
    }
}
