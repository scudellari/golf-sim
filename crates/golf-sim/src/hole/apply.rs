//! Resolve actions and commit live or recorded entries.

use golf_sim_base::{
    ClubId, Error, PromptKind, Real, Seed, Vec2, Vec3,
    corpus::first_difference,
    log,
    trace::{NoProbe, Phase, Probe, Source, Trace},
    variant::{Dict, Variant},
};
use golf_sim_conditions::{lies::BallLie, world::World};
use golf_sim_course::legal::Area;
use golf_sim_physics::facade::standard_club;
use golf_sim_rules::{
    relief::{self, AbnormalKind, ReliefArea},
    rulings::{Consequence, ReliefOption, RulesStep, Status},
    transitions,
};
use golf_sim_stroke::{
    address::{Address, Setting},
    delivery::DeliveryInput,
    results::{DropResult, StrokeResult},
    stroke::{self, StrokeInput},
};
use golf_sim_swing::{
    execution::Execution,
    hands::{Profile, Swing},
    session::SwingSession,
    spec::SwingSpec,
};

use super::{Hole, Step, relief_ground};
use crate::{
    action::{Action, Aim, StrokeChoice},
    records::hole::{
        DecisionEntry, DecisionKind, Entry, Fields, Refusal, StrokeEntry, StrokeFacts,
    },
    setup::{dictionary, vector3},
};

const SETTLE_USEC: i64 = 250_000;
const CLOCK_STEP_S: Real = 150.0;
const KNEE_HEIGHT_M: Real = 0.50;
const RELIEF_BALL_RADIUS_M: Real = 0.02135;
const NO_POINT: Vec3 = Vec3::new(Real::INFINITY, Real::INFINITY, Real::INFINITY);

struct CommittedStroke {
    clock_s: Real,
    lie: BallLie,
    rules: RulesStep,
    result: StrokeResult,
}

impl Hole {
    pub(super) fn resolve(&self, action: Action) -> Result<Entry, Refusal> {
        let (kind, option, point, footprint) = match action {
            Action::Stroke(choice) => return self.stroke_entry(choice),
            Action::GoForward => (
                DecisionKind::GoForward,
                ReliefOption::Cancel,
                NO_POINT,
                vec![],
            ),
            Action::Relief { option, footprint } => {
                (DecisionKind::Relief, option, NO_POINT, footprint)
            }
            Action::Drop(point) => (DecisionKind::Drop, ReliefOption::Cancel, point, vec![]),
            Action::Place(point) => (DecisionKind::Place, ReliefOption::Cancel, point, vec![]),
        };
        Ok(Entry::Decision(DecisionEntry {
            kind,
            option: option as i64,
            point,
            footprint,
            ruling: Dict::default(),
        }))
    }

    fn stroke_entry(&self, choice: StrokeChoice) -> Result<Entry, Refusal> {
        for setting in Setting::ALL {
            if !choice.address.value(setting).is_finite() {
                return Err(invalid(setting.key(), "expected a finite setting"));
            }
        }
        let aim = match choice.aim {
            Aim::At(point) => point - self.ball_lie.spot,
            Aim::Along(direction) => direction,
        };
        if !aim.is_finite() {
            return Err(invalid("aim", "expected a finite aim"));
        }
        let aim = Vec3::new(aim.x, 0.0, aim.z).normalized();
        if !aim.is_finite() || aim == Vec3::ZERO {
            return Err(invalid("aim", "expected a horizontal direction"));
        }

        let club = choice.club;
        let address = choice.address.fit(club);
        let flagstick_in = choice
            .flagstick_in
            .unwrap_or_else(|| self.ball_lie.surface != golf_sim_base::Surface::Green);
        let world = self.world_at(self.clock_s);
        let input = self.stroke_input(club, &address, aim, flagstick_in, &world);
        let spec = stroke::swing_spec(&input, self.record.setup.settings.assists());
        validate_swing(choice.swing, &spec)?;
        let execution = choice
            .swing
            .execute(&spec, SETTLE_USEC, input.delivery.attempt_seed);
        let gust_time_s = gust_time(execution.impact_usec, 0)?;
        Ok(Entry::Stroke(Box::new(StrokeEntry {
            facts: StrokeFacts {
                attempt: self.attempt as i64,
                club_id: club,
                address,
                slot: 0,
                shape: 0,
                height: 0,
                aim,
                flagstick_in,
                execution,
                gust_time_s,
                clock_s: self.clock_s,
                rest: self.ball_lie.spot,
                lie: self.ball_lie,
            },
            swing: StrokeEntry::swing_dict(choice.swing),
            address_usec: Some(SETTLE_USEC),
        })))
    }

    pub(super) fn apply(&mut self, mut entry: Entry, replay: bool) -> Result<Step, Refusal> {
        let _span = log::hole(self.record.setup.hole as u32).entered();
        let stroke = match &mut entry {
            Entry::Stroke(stroke) => Some(self.apply_stroke(stroke, replay)?),
            Entry::Decision(decision) => {
                self.apply_decision(decision, replay)?;
                None
            }
        };
        self.record.entries.push(entry);
        Ok(Step { stroke })
    }

    fn apply_stroke(
        &mut self,
        entry: &mut StrokeEntry,
        replay: bool,
    ) -> Result<StrokeResult, Refusal> {
        if entry.facts.attempt != self.attempt as i64 {
            return Err(self.diverged("attempt"));
        }
        let prompt = self.rules.prompt().kind;
        if prompt != PromptKind::Play {
            return Err(self.rejected(replay, "stroke", format!("prompt is {prompt:?}")));
        }
        let _span = log::stroke(self.attempt as u32, entry.facts.club_id).entered();
        let world = self.world_at(self.clock_s);
        let facts = &entry.facts;
        let mut input = self.stroke_input(
            facts.club_id,
            &facts.address,
            facts.aim,
            facts.flagstick_in,
            &world,
        );
        if replay {
            let spec = stroke::swing_spec(&input, self.record.setup.settings.assists());
            let (execution, gust) = replay_swing(entry, &spec, input.delivery.attempt_seed)?;
            self.compare(
                "execution",
                execution_value(entry.facts.execution),
                execution_value(execution),
            )?;
            self.compare("gust_time_s", entry.facts.gust_time_s.into(), gust.into())?;
        }
        input.gust_time_s = entry.facts.gust_time_s;
        let execution = entry.facts.execution;
        let (committed, trace) = if self.trace_limit == 0 {
            (self.play(&input, execution, &world, &mut NoProbe), None)
        } else {
            let mut trace = Trace::default();
            let committed = self.play(&input, execution, &world, &mut trace);
            (committed, Some(trace))
        };
        let CommittedStroke {
            clock_s,
            lie,
            rules,
            result,
        } = committed;
        if replay {
            self.compare("clock_s", entry.facts.clock_s.into(), clock_s.into())?;
            self.compare(
                "rest",
                vector3(entry.facts.rest),
                vector3(result.outcome.rest),
            )?;
            self.compare("lie", lie_value(entry.facts.lie), lie_value(lie))?;
        }
        if let Some(refusal) = rules.refusal {
            return Err(self.rejected(replay, "stroke", format!("{refusal:?}")));
        }

        entry.facts.clock_s = clock_s;
        entry.facts.rest = result.outcome.rest;
        entry.facts.lie = lie;
        self.clock_s = clock_s;
        self.attempt += 1;
        self.last_lie = self.ball_lie;
        self.rest_lie = lie;
        self.follow(rules);
        if let Some(trace) = trace {
            if self.traces.len() == self.trace_limit {
                self.traces.pop_front();
            }
            self.traces.push_back(trace);
        }
        Ok(result)
    }

    fn stroke_input(
        &self,
        club: ClubId,
        address: &Address,
        aim: Vec3,
        flagstick_in: bool,
        world: &World<'_>,
    ) -> StrokeInput {
        let footprint = address.footprint(club);
        StrokeInput {
            delivery: DeliveryInput {
                club,
                address: address.clone(),
                lie: world.address_lie(self.ball_lie, &footprint, aim),
                attempt_seed: self
                    .record
                    .setup
                    .seed
                    .derive(&format!("attempt/{}", self.attempt)),
            },
            aim,
            gust_time_s: 0.0,
            flagstick_in,
        }
    }

    fn play<P: Probe>(
        &self,
        input: &StrokeInput,
        execution: Execution,
        world: &World<'_>,
        probe: &mut P,
    ) -> CommittedStroke {
        let result = stroke::play(input, execution, world, probe);
        // The constant day has no wear overlay. Marks accompany the result for a generated day.
        let clock_s = self.clock_s + CLOCK_STEP_S;
        let next = self.world_at(clock_s);
        let lie = next.settle(
            result.outcome.rest,
            input.delivery.attempt_seed.derive("sit"),
            probe,
        );
        let rules = transitions::after_stroke(
            &self.rules,
            &result.outcome,
            next.legal().expect("course world"),
            next.cup_position(),
            Some(self.attempt),
            false,
        );
        if P::ENABLED
            && let Some(ruling) = rules.rules.last()
        {
            trace_ruling(
                probe,
                ruling.status,
                ruling.consequence,
                ruling.penalty_strokes,
            );
        }
        CommittedStroke {
            clock_s,
            lie,
            rules,
            result,
        }
    }

    fn apply_decision(&mut self, entry: &mut DecisionEntry, replay: bool) -> Result<(), Refusal> {
        let world = self.world_at(self.clock_s);
        let legal = world.legal().expect("course world");
        let option = relief_option(entry.option)
            .ok_or_else(|| self.rejected(replay, "option", "unknown relief option"))?;
        if entry.kind != DecisionKind::Relief && option != ReliefOption::Cancel {
            return Err(self.rejected(replay, "option", "unexpected relief option"));
        }
        if !matches!(entry.footprint.len(), 0 | 3)
            || entry.footprint.iter().any(|offset| {
                (!offset.x.is_finite() || !offset.y.is_finite()) || offset.length() > 2.0
            })
        {
            return Err(self.rejected(replay, "footprint", "invalid stance footprint"));
        }
        let mut drop = DropResult {
            first: None,
            rest: None,
        };
        let step = match entry.kind {
            DecisionKind::GoForward => {
                let area = self.relief_area(ReliefOption::StrokeAndDistance, &world);
                let from = self.previous_area(&area, &world);
                transitions::go_forward(&self.rules, Some(area), from)
            }
            DecisionKind::Relief => self.choose_relief(option, &entry.footprint, &world),
            DecisionKind::Drop => {
                let prompt = self.rules.prompt();
                let area = prompt.ruling.and_then(|ruling| ruling.relief.as_ref());
                if prompt.kind != PromptKind::Drop
                    || !entry.point.is_finite()
                    || !area.is_some_and(|area| area.accepts_first(entry.point, legal))
                {
                    return Err(self.rejected(
                        replay,
                        "point",
                        "drop point is outside the offered relief area",
                    ));
                }
                drop = stroke::drop(entry.point, KNEE_HEIGHT_M, &world, &mut NoProbe);
                relief::after_drop(&self.rules, &drop, legal, &mut NoProbe)
            }
            DecisionKind::Place => relief::place(&self.rules, entry.point, &mut NoProbe),
        };
        if let Some(refusal) = step.refusal {
            return Err(self.rejected(replay, "ruling", format!("{refusal:?}")));
        }
        let ruling = step.rules.last().expect("a decision leaves a ruling");
        if step.prompt().kind == PromptKind::Drop
            && ruling.relief.as_ref().is_some_and(|area| {
                relief_ground::drop_spots(area, &world, &self.course).is_empty()
            })
        {
            return Err(self.rejected(replay, "ruling", "no legal drop spot"));
        }
        let placed = if matches!(entry.kind, DecisionKind::Drop | DecisionKind::Place)
            && matches!(
                ruling.consequence,
                Consequence::Relief | Consequence::ProvisionalPlay
            ) {
            let seed = self
                .record
                .setup
                .seed
                .derive(&format!("relief/{}", self.record.entries.len()));
            ruling
                .spot
                .map(|spot| stroke::place(spot, &world, seed, &mut NoProbe))
        } else {
            None
        };
        let spot = |point: Option<Vec3>| vector3(point.unwrap_or(NO_POINT));
        let facts = dictionary([
            ("next", (ruling.consequence as i64).into()),
            ("status", (ruling.status as i64).into()),
            ("penalty", (ruling.penalty_strokes as i64).into()),
            ("strokes", (step.rules.strokes() as i64).into()),
            ("spot", spot(ruling.spot)),
            (
                "reference",
                spot(ruling.relief.as_ref().map(|area| area.reference)),
            ),
            ("place_at", spot(ruling.place_at)),
            ("first", spot(drop.first)),
            ("rest", spot(drop.rest)),
            (
                "lie",
                placed.map_or_else(|| Variant::Dict(Dict::default()), lie_value),
            ),
        ]);
        if replay {
            self.compare(
                "ruling",
                Variant::Dict(entry.ruling.clone()),
                Variant::Dict(facts),
            )?;
        } else {
            entry.ruling = facts;
        }

        if option == ReliefOption::Provisional
            && self
                .rules
                .last()
                .is_some_and(|last| last.provisional.is_none())
        {
            self.original_lie = Some(self.rest_lie);
        }
        if let Some(lie) = placed {
            self.rest_lie = lie;
        }
        self.follow(step);
        Ok(())
    }

    fn relief_area(&self, option: ReliefOption, world: &World<'_>) -> ReliefArea {
        let setup = &self.record.setup;
        let hole = self.course.hole(setup.hole).expect("checked hole");
        ReliefArea {
            option,
            hole: world.cup_position().expect("checked cup"),
            club_length_m: standard_club(ClubId::Driver).length_m,
            tee: relief_ground::tee_outline(setup, &self.course).to_vec(),
            green: hole.green.outline.clone(),
            ball_radius_m: RELIEF_BALL_RADIUS_M,
            reference: NO_POINT,
            bunker: Vec::new(),
            penalty_area: None,
            required_area: None,
            green_placement: false,
            condition: None,
            footprint: Vec::new(),
        }
    }

    fn previous_area(&self, area: &ReliefArea, world: &World<'_>) -> Area {
        self.rules.last().map_or(Area::General, |last| {
            world
                .legal()
                .expect("course world")
                .area_at(last.from, &area.tee, &area.green)
        })
    }

    fn choose_relief(
        &self,
        option: ReliefOption,
        footprint: &[Vec2],
        world: &World<'_>,
    ) -> RulesStep {
        let legal = world.legal().expect("course world");
        let mut area = self.relief_area(option, world);
        let from = self.previous_area(&area, world);
        if option == ReliefOption::Provisional {
            return transitions::declare_provisional(&self.rules, area, from);
        }
        let current = self
            .rules
            .ball()
            .and_then(|spot| legal.penalty_at(spot, area.ball_radius_m));
        let previous = self
            .rules
            .last()
            .and_then(|last| last.penalty_area.as_ref());
        let penalty = matches!((current, previous), (Some(current), Some(previous))
            if current.outline == previous.outline && current.color == previous.color);
        if self.rules.prompt().kind == PromptKind::Choose || penalty {
            return relief::choose_penalty(&self.rules, area, from, &mut NoProbe);
        }
        if option == ReliefOption::Embedded {
            return relief::embedded_relief(&self.rules, area, legal, &mut NoProbe);
        }
        let condition = match option {
            ReliefOption::TemporaryWater => Some(AbnormalKind::TemporaryWater),
            ReliefOption::GroundUnderRepair => Some(AbnormalKind::GroundUnderRepair),
            ReliefOption::ImmovableObstruction => Some(AbnormalKind::ImmovableObstruction),
            ReliefOption::BunkerAbnormalOutside => None,
            _ => return relief::declare_unplayable(&self.rules, area, legal, from, &mut NoProbe),
        };
        area.footprint = footprint.to_vec();
        if let Some(ball) = self.rules.ball() {
            area.condition = relief_ground::conditions(&self.course, ball, Default::default())
                .into_iter()
                .find(|candidate| {
                    condition.is_none_or(|kind| kind == candidate.kind)
                        && relief::relief_available(&self.rules, legal, &area, candidate)
                });
        }
        relief::ask_relief(&self.rules, area, legal, &mut NoProbe)
    }

    fn follow(&mut self, step: RulesStep) {
        if let Some(previous) = self.rules.last()
            && previous.provisional.is_some()
            && step
                .rules
                .last()
                .is_some_and(|last| last.provisional.is_none())
        {
            let original = self.original_lie.take();
            if previous.status == Status::Doubtful
                && let Some(lie) = original
            {
                self.rest_lie = lie;
            }
        }
        let prompt = step.prompt();
        if prompt.kind == PromptKind::Play {
            self.ball_lie = if prompt.from_last_spot() {
                self.last_lie
            } else {
                self.rest_lie
            };
        }
        self.rules = step.rules;
    }

    fn compare(&self, field: &str, saved: Variant, actual: Variant) -> Result<(), Refusal> {
        if let Some(path) = first_difference(&saved, &actual) {
            return Err(self.diverged(if path == "$" {
                field.into()
            } else {
                format!("{field}.{path}")
            }));
        }
        Ok(())
    }

    fn diverged(&self, path: impl Into<String>) -> Refusal {
        Error::Diverged {
            entry: self.record.entries.len(),
            path: path.into(),
        }
        .into()
    }

    fn rejected(&self, replay: bool, path: &str, message: impl Into<String>) -> Refusal {
        if replay {
            self.diverged(path)
        } else {
            invalid(path, message)
        }
    }
}

fn relief_option(index: i64) -> Option<ReliefOption> {
    use ReliefOption::*;
    [
        Cancel,
        StrokeAndDistance,
        BackOnLine,
        Lateral,
        OutsideBunker,
        TemporaryWater,
        GroundUnderRepair,
        ImmovableObstruction,
        Embedded,
        BunkerAbnormalOutside,
        Provisional,
    ]
    .get(usize::try_from(index).ok()?)
    .copied()
}

fn validate_swing(swing: Swing, spec: &SwingSpec) -> Result<(), Refusal> {
    match swing {
        Swing::Exact {
            power,
            timing_fault,
            strike_m,
        } => {
            if !(0.0..=spec.power_cap).contains(&power) {
                return Err(invalid("swing.power", "outside the meter's range"));
            }
            let window_s = spec.window_early_s.max(spec.window_late_s);
            let error_usec = timing_fault.abs() * window_s * 1_000_000.0;
            if !error_usec.is_finite() || error_usec >= (i64::MAX / 4) as Real {
                return Err(invalid("swing.timing_fault", "outside the supported range"));
            }
            if strike_m.is_some_and(|strike| !strike.x.is_finite() || !strike.y.is_finite()) {
                return Err(invalid("swing.strike_m", "expected a finite strike"));
            }
        }
        Swing::Skill(profile) => {
            if ![
                profile.sigma_p_ms,
                profile.bias_t_ms,
                profile.sigma_t_ms,
                profile.sigma_s_mm,
                profile.tau_ms,
            ]
            .into_iter()
            .all(Real::is_finite)
            {
                return Err(invalid("swing.profile", "expected finite profile values"));
            }
        }
    }
    Ok(())
}

fn replay_swing(
    entry: &StrokeEntry,
    spec: &SwingSpec,
    seed: Seed,
) -> Result<(Execution, Real), Refusal> {
    let fields = Fields::new(&entry.swing, "swing");
    let mode = fields.string("mode")?;
    let execution = if mode == "pressed" {
        pressed_execution(entry, spec, seed)?
    } else {
        let start = entry
            .address_usec
            .ok_or_else(|| invalid("address_usec", "missing swing start"))?;
        if start > i64::MAX / 2 {
            return Err(invalid("address_usec", "swing start is out of range"));
        }
        let swing = if mode == "exact" {
            Swing::Exact {
                power: fields.real("power")?,
                timing_fault: fields.real("timing_fault")?,
                strike_m: read_vec2(&entry.swing, "strike_m"),
            }
        } else {
            let values = entry
                .swing
                .get("profile")
                .and_then(Variant::as_dict)
                .and_then(|profile| profile.get("values"))
                .and_then(Variant::as_dict)
                .expect("decoded profile");
            let fields = Fields::new(values, "swing.profile.values");
            Swing::Skill(Profile {
                name: "",
                sigma_p_ms: fields.real("sigma_p_ms")?,
                bias_t_ms: fields.real("bias_t_ms")?,
                sigma_t_ms: fields.real("sigma_t_ms")?,
                sigma_s_mm: fields.real("sigma_s_mm")?,
                tau_ms: fields.real("tau_ms")?,
            })
        };
        validate_swing(swing, spec)?;
        swing.execute(spec, start, seed)
    };
    let origin = if mode == "pressed" {
        entry.address_usec.unwrap_or(0)
    } else {
        0
    };
    Ok((execution, gust_time(execution.impact_usec, origin)?))
}

fn pressed_execution(
    entry: &StrokeEntry,
    spec: &SwingSpec,
    seed: Seed,
) -> Result<Execution, Refusal> {
    let Variant::Array(presses) = entry.swing.get("presses").expect("decoded presses") else {
        unreachable!("decoded presses");
    };
    let Some((Variant::Int(start), inputs)) = presses.items.split_first() else {
        return Err(invalid("swing.presses", "missing start press"));
    };
    let stamp = |value| {
        (0..=i64::MAX / 2)
            .contains(&value)
            .then_some(value)
            .ok_or_else(|| invalid("swing.presses", "stamp is out of range"))
    };
    let mut session = SwingSession::new(*spec, stamp(start.value())?, stroke::reticle_seed(seed));
    for input in inputs {
        match input {
            Variant::Int(time) => session.press(stamp(time.value())?),
            Variant::Dict(steer) => {
                let fields = Fields::new(steer, "swing.presses");
                let delta = read_vec2(steer, "delta_m").expect("decoded steer");
                session.steer(stamp(fields.integer("usec")?)?, delta);
            }
            _ => unreachable!("decoded press or steer"),
        }
    }
    // A swing with no final press ends at the meter's timeout.
    session.advance(i64::MAX);
    session
        .execution()
        .ok_or_else(|| invalid("swing.presses", "swing has no impact"))
}

fn gust_time(impact_usec: i64, origin_usec: i64) -> Result<Real, Refusal> {
    let elapsed = impact_usec
        .checked_sub(origin_usec)
        .ok_or_else(|| invalid("execution.impact_usec", "impact time is out of range"))?;
    Ok(elapsed.max(0) as Real / 1_000_000.0)
}

fn read_vec2(dict: &Dict, key: &str) -> Option<Vec2> {
    if let Some(Variant::Vector2(value)) = dict.get(key) {
        let [x, y] = value.values;
        Some(Vec2::new(x, y))
    } else {
        None
    }
}

fn execution_value(execution: Execution) -> Variant {
    Variant::Dict(dictionary([
        ("power", execution.power.into()),
        ("timing_error_s", execution.timing_error_s.into()),
        ("timing_fault", execution.timing_fault.into()),
        ("timed_out", Variant::Bool(execution.timed_out)),
        (
            "strike_m",
            execution.strike_m.map_or(Variant::Nil, |point| {
                Variant::Vector2(golf_sim_base::variant::Components {
                    values: [point.x, point.y],
                    width: golf_sim_base::variant::FloatWidth::Double,
                })
            }),
        ),
        ("impact_usec", execution.impact_usec.into()),
        ("backswing_s", execution.backswing_s.into()),
        ("ring_s", execution.ring_s.into()),
    ]))
}

fn lie_value(lie: BallLie) -> Variant {
    Variant::Dict(dictionary([
        ("spot", vector3(lie.spot)),
        ("surface", (lie.surface as i64).into()),
        ("sit", (lie.sit as i64).into()),
        ("ball_height_m", lie.ball_height_m.into()),
        ("grass_height_m", lie.grass_height_m.into()),
        ("grass_density", lie.grass_density.into()),
        ("sand_firmness", lie.sand_firmness.into()),
        ("divot", (lie.divot as i64).into()),
    ]))
}

fn trace_ruling(probe: &mut impl Probe, status: Status, next: Consequence, penalty: usize) {
    probe.begin_phase(Phase::Ruling);
    probe.value("status", status as u8 as Real, "", Source::Computed);
    probe.value("next", next as u8 as Real, "", Source::Computed);
    probe.value("penalty", penalty as Real, "strokes", Source::Computed);
}

fn invalid(path: &str, message: impl Into<String>) -> Refusal {
    Error::Malformed {
        path: path.into(),
        message: message.into(),
    }
    .into()
}
