use golf_sim::{
    action::{Action, ReliefOption, StrokeChoice, Swing},
    base::{ClubId, PromptKind, Real, Seed, Surface, Vec2, Vec3, fm},
    conditions::lies::{AddressLie, BallLie, Sit},
    hole::Hole,
    observe::{LieReading, Observation, Score},
    physics::values::units::YARD,
    setup::Mode,
    stroke::{
        address::Setting,
        delivery::DeliveryInput,
        results::Preview,
        stroke::{self, StrokeInput},
        tuning::SPEED_AT_ZERO_POWER,
    },
};

use crate::protocol::ProfileName;

const STROKE_LIMIT: usize = 20;
const PLAYS_LIKE_STEP_M: Real = 5.0 * YARD;
const RELIEF_ORDER: [ReliefOption; 3] = [
    ReliefOption::Lateral,
    ReliefOption::BackOnLine,
    ReliefOption::StrokeAndDistance,
];

#[derive(Clone, Copy)]
pub enum Hands {
    Exact,
    Profile(ProfileName),
}

pub struct Played {
    pub score: Score,
    pub holed: bool,
}

pub fn play(hole: &mut Hole, mode: Mode, hands: Hands) -> Result<Played, String> {
    let mut golfer = Golfer::default();
    loop {
        let view = hole.observe(mode);
        if view.prompt == PromptKind::Done || view.score.strokes >= STROKE_LIMIT {
            return Ok(Played {
                score: view.score,
                holed: view.prompt == PromptKind::Done,
            });
        }
        let action = golfer.decide(hole, &view, hands)?;
        hole.act(action).map_err(|error| error.to_string())?;
    }
}

struct Golfer {
    last: Option<Shot>,
    reliefs_here: usize,
    tee_retries: usize,
    putt_scale: Real,
}

struct Shot {
    ball: LieReading,
    penalties: usize,
    club: ClubId,
    length: Real,
    chip: bool,
}

impl Default for Golfer {
    fn default() -> Self {
        Self {
            last: None,
            reliefs_here: 0,
            tee_retries: 0,
            putt_scale: 24.0,
        }
    }
}

impl Golfer {
    fn decide(&mut self, hole: &Hole, view: &Observation, hands: Hands) -> Result<Action, String> {
        match view.prompt {
            PromptKind::Play => {}
            PromptKind::MaybeLost => return Ok(Action::GoForward),
            PromptKind::Choose => {
                return choose_relief(hole, 0)
                    .map(|(_, action)| action)
                    .ok_or_else(|| "no usable relief offered".into());
            }
            _ => {
                return hole
                    .legal()
                    .into_iter()
                    .next()
                    .ok_or_else(|| format!("no action offered at {:?}", view.prompt));
            }
        }

        let ball = view.ball.ok_or("no ball at address")?;
        let address = view.address.as_ref().ok_or("no club at address")?;
        let mut previous_chip = None;
        if let Some(last) = &self.last {
            let report = view
                .last_stroke
                .as_ref()
                .ok_or("no previous stroke report")?;
            let moved = (point(ball.spot) - point(last.ball.spot)).length() >= 1.0;
            if !moved
                && ball.surface != Surface::Green
                && view.score.penalties == last.penalties
                && view.relief_available
                && let Some((index, action)) = choose_relief(hole, self.reliefs_here)
            {
                self.reliefs_here = (index + 1).min(RELIEF_ORDER.len() - 1);
                return Ok(action);
            }
            if view.score.penalties == last.penalties && moved {
                self.reliefs_here = 0;
            }
            if ball.surface == Surface::Tee && view.score.penalties > last.penalties {
                self.tee_retries += 1;
            }
            if last.club == ClubId::Putter && report.total.exact_m > 0.15 {
                let power = report.power.max(0.05);
                self.putt_scale += 0.7 * (report.total.exact_m / (power * power) - self.putt_scale);
            }
            if last.chip {
                previous_chip = Some((report.power.clamp(0.05, 1.0), report.total.exact_m));
            }
        }

        let distance = view.to_flag.ok_or("no distance to the flag")?.exact_m;
        let mut aim = point(address.aim);
        if ball.surface == Surface::Tee && self.tee_retries > 0 {
            let turn = 12.0 * self.tee_retries.div_ceil(2) as Real;
            let sign = if self.tee_retries.is_multiple_of(2) {
                -1.0
            } else {
                1.0
            };
            aim = turn_aim(aim, (sign * turn).to_radians());
        }
        let club = if view.mode == Mode::Tour && !address.putting && ball.surface != Surface::Tee {
            if distance < 78.0 {
                ClubId::SandWedge
            } else if distance < 170.0 {
                ClubId::Iron7
            } else {
                ClubId::Hybrid
            }
        } else {
            address.club
        };
        let mut choice =
            StrokeChoice::stock(club, point(address.aim_point)).with_aim_direction(aim);
        if let Some(last) = self.last.as_ref().filter(|last| last.club == club) {
            choice.address.swing_length = last.length;
        }
        let putting = club == ClubId::Putter;
        choice.flagstick_in = Some(ball.surface != Surface::Green);
        let (mut power, preview) = if view.mode == Mode::Caddie {
            fit_caddie(hole, &mut choice, distance)?
        } else {
            (self.fit_tour(&mut choice, ball, distance)?, None)
        };
        if putting && let Some(preview) = preview {
            choice = choice.with_aim_direction(read_putt(
                &preview,
                aim,
                point(ball.spot),
                point(view.cup),
                distance,
            ));
        }
        let chip = !putting && distance < 18.0;
        if chip
            && let Some((previous_power, travel)) = previous_chip
            && travel > distance + 1.0
        {
            // Below the mark, the speed of a chip still has a nonzero floor.
            let floor = SPEED_AT_ZERO_POWER;
            let speed = floor + (1.0 - floor) * previous_power;
            power = power.min((speed * fm::sqrt(distance / travel) - floor) / (1.0 - floor));
        }
        power = power.clamp(0.05, 1.0);
        choice.swing = match hands {
            Hands::Exact => Swing::Exact {
                power,
                timing_fault: 0.0,
                strike_m: Some(Vec2::ZERO),
            },
            Hands::Profile(profile) => {
                // Skill swings press at the address's target power.
                let [min, _] = Setting::SwingLength.range(club);
                choice.address.swing_length = power.max(min);
                profile.swing()
            }
        };
        self.last = Some(Shot {
            ball,
            penalties: view.score.penalties,
            club,
            length: choice.address.swing_length,
            chip,
        });
        Ok(Action::Stroke(choice))
    }

    fn fit_tour(
        &self,
        choice: &mut StrokeChoice,
        ball: LieReading,
        distance: Real,
    ) -> Result<Real, String> {
        if choice.club == ClubId::Putter {
            return Ok(fm::sqrt((distance + 0.35) / self.putt_scale));
        }
        let measure = |choice: &StrokeChoice| Ok((stock_carry(choice, ball), ()));
        let initial = measure(choice)?;
        fit_length(choice, carry_target(distance), initial, measure).map(|(power, ())| power)
    }
}

fn choose_relief(hole: &Hole, first: usize) -> Option<(usize, Action)> {
    let legal = hole.legal();
    RELIEF_ORDER
        .iter()
        .enumerate()
        .skip(first)
        .find_map(|(index, wanted)| {
            legal
                .iter()
                .find(|action| matches!(action, Action::Relief { option, .. } if option == wanted))
                .cloned()
                .map(|action| (index, action))
        })
}

fn fit_caddie(
    hole: &Hole,
    choice: &mut StrokeChoice,
    distance: Real,
) -> Result<(Real, Option<Preview>), String> {
    let putting = choice.club == ClubId::Putter;
    let measure = |choice: &StrokeChoice| {
        let preview = hole.preview(choice).map_err(|error| error.to_string())?;
        Ok((nominal_reach(&preview, putting), preview))
    };
    let initial = measure(choice)?;
    let preview = &initial.1;
    let wanted = if putting {
        distance + 0.35
    } else {
        carry_target(plays_like_read(preview.plays_like_m, distance))
    };
    let (power, preview) = fit_length(choice, wanted, initial, measure)?;
    Ok((power, putting.then_some(preview)))
}

fn fit_length<T>(
    choice: &mut StrokeChoice,
    wanted: Real,
    (mut reach, mut reading): (Real, T),
    measure: impl Fn(&StrokeChoice) -> Result<(Real, T), String>,
) -> Result<(Real, T), String> {
    let setting = Setting::SwingLength;
    let [min, max] = setting.range(choice.club);
    let mut length = choice.address.swing_length;
    let direction = if reach < wanted { 1.0 } else { -1.0 };
    while reach.is_finite() && reach != wanted {
        let next = (length + direction * setting.step_size()).clamp(min, max);
        if (next - length).abs() < 1e-9 {
            break;
        }
        choice.address.swing_length = next;
        let (trial_reach, trial) = measure(choice)?;
        if !trial_reach.is_finite()
            || (direction < 0.0 && (trial_reach < wanted || trial_reach >= reach))
            || (direction > 0.0 && trial_reach <= reach)
        {
            break;
        }
        length = next;
        reading = trial;
        reach = trial_reach;
        if direction > 0.0 && reach >= wanted {
            break;
        }
    }
    choice.address.swing_length = length;
    let power = if reach > wanted {
        length * fm::sqrt(wanted / reach)
    } else {
        length
    };
    Ok((power, reading))
}

fn nominal_reach(preview: &Preview, putting: bool) -> Real {
    if putting {
        preview.roll_m
    } else if !preview.target_distance_m.is_finite() {
        preview.carry_m
    } else if !preview.plays_like_m.is_finite() {
        (preview.carry_m / PLAYS_LIKE_STEP_M).round() * PLAYS_LIKE_STEP_M
    } else {
        let plays = preview.plays_like_m;
        preview.carry_to_target_m * plays / plays_like_read(plays, preview.target_distance_m)
    }
}

fn plays_like_read(plays: Real, distance: Real) -> Real {
    if (plays - distance).abs() >= 0.5 * PLAYS_LIKE_STEP_M {
        (plays / PLAYS_LIKE_STEP_M).round() * PLAYS_LIKE_STEP_M
    } else {
        distance
    }
}

fn carry_target(distance: Real) -> Real {
    distance * if distance < 40.0 { 0.8 } else { 0.94 }
}

fn point([x, y, z]: [Real; 3]) -> Vec3 {
    Vec3::new(x, y, z)
}

fn turn_aim(aim: Vec3, angle: Real) -> Vec3 {
    let side = Vec3::new(-aim.z, 0.0, aim.x);
    aim * fm::cos(angle) + side * fm::sin(angle)
}

fn read_putt(preview: &Preview, aim: Vec3, ball: Vec3, cup: Vec3, distance: Real) -> Vec3 {
    if distance < 0.2 || preview.putt_line.is_empty() {
        return aim;
    }
    let side = Vec3::new(-aim.z, 0.0, aim.x);
    let mut drift = 0.0;
    for &point in &preview.putt_line {
        let from_ball = point - ball;
        drift = from_ball.dot(side);
        if from_ball.dot(aim) >= distance {
            break;
        }
    }
    let correction = fm::atan2((cup - ball).dot(side) - drift, distance);
    if correction.abs() > 0.001 {
        turn_aim(aim, correction)
    } else {
        aim
    }
}

fn stock_carry(choice: &StrokeChoice, ball: LieReading) -> Real {
    // Tour has the club's stock yardage on level ground, independent of the lie.
    let surface = if ball.surface == Surface::Tee {
        Surface::Tee
    } else {
        Surface::Fairway
    };
    let input = StrokeInput::new(
        DeliveryInput {
            club: choice.club,
            address: choice.address.clone(),
            lie: AddressLie {
                ball: BallLie::on(surface, Sit::Normal, point(ball.spot)),
                slope_along_rad: 0.0,
                slope_across_rad: 0.0,
                ball_slope_along_rad: 0.0,
                lead_foot_surface: surface,
                trail_foot_surface: surface,
                club_entry_surface: surface,
            },
            attempt_seed: Seed(0),
        },
        Vec3::new(0.0, 0.0, -1.0),
    );
    stroke::stock_carry(&input)
}
