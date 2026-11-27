//! First aim, club advice and a nominal preview of a chosen address.

use golf_sim_base::{ClubId, Error, PromptKind, Real, Seed, Surface, Vec3};
use golf_sim_conditions::{
    lies::{BallLie, Sit},
    world::World,
};
use golf_sim_stroke::{
    address::{Address, Setting},
    bag,
    delivery::DeliveryInput,
    results::Preview,
    stroke::{self, StrokeInput},
};

use super::{Hole, prompt::ActionRefusal};
use crate::{
    action::{Aim, StrokeChoice},
    records::hole::Entry,
    setup::Mode,
};

const REACH_M: Real = 230.0;
const CARRY_SHARE: Real = 0.95;
const SEARCH_STEPS: usize = 4;
const PITCH_M: Real = 35.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Advice {
    pub club: ClubId,
    /// Horizontal unit direction before any player adjustment or restored aim.
    pub first_aim: Vec3,
}

impl Hole {
    pub fn advice(&self) -> Option<Advice> {
        let prompt = self.prompt();
        if prompt.kind != PromptKind::Play {
            return None;
        }
        let world = self.world_at(self.clock_s);
        let cup = world.cup_position().expect("checked cup");
        let first_aim = self.first_aim(cup);
        let ball = self.ball_lie;
        let distance = flat(cup - ball.spot).length();
        let previous = || self.previous_club().unwrap_or(ClubId::Iron7);
        let club = if prompt.from_last_spot() {
            previous()
        } else if ball.surface == Surface::Green {
            ClubId::Putter
        } else if ball.is_teed() && distance > REACH_M {
            ClubId::Driver
        } else if self.record.setup.settings.info_mode == Mode::Tour {
            previous()
        } else if distance < PITCH_M {
            ClubId::SandWedge
        } else {
            self.caddie_club(&world, first_aim, cup, distance)
        };
        Some(Advice { club, first_aim })
    }

    /// Preview the fitted address in mean air, using a nominal swing. An aim
    /// point sets the target height. With an aim direction the target is the cup.
    pub fn preview(&self, choice: &StrokeChoice) -> Result<Preview, ActionRefusal> {
        if self.record.setup.settings.info_mode != Mode::Caddie {
            return Err(ActionRefusal::PreviewUnavailable);
        }
        let prompt = self.prompt().kind;
        if prompt != PromptKind::Play {
            return Err(ActionRefusal::Rules(golf_sim_base::Refusal::NotNow {
                prompt,
            }));
        }
        for setting in Setting::ALL {
            if !choice.address.value(setting).is_finite() {
                return Err(invalid(setting.key(), "expected a finite setting"));
            }
        }
        let ball = self.ball_lie;
        let world = self.world_at(self.clock_s);
        let (direction, target) = match choice.aim {
            Aim::At(point) => (point - ball.spot, Some(point)),
            Aim::Along(direction) => (direction, world.cup_position()),
        };
        let aim = flat(direction).normalized();
        if !direction.is_finite() || !aim.is_finite() || aim == Vec3::ZERO {
            return Err(invalid("aim", "expected a finite horizontal direction"));
        }
        let club = choice.club;
        let mut input = nominal_input(&world, ball, club, choice.address.clone().fit(club), aim);
        if let Some(flagstick_in) = choice.flagstick_in {
            input.flagstick_in = flagstick_in;
        }
        Ok(stroke::preview(&input, &world, target, None))
    }

    pub(super) fn first_aim(&self, cup: Vec3) -> Vec3 {
        let ball = self.ball_lie;
        let tee = self.record.setup.tee(&self.course).expect("checked tee");
        let to_cup = flat(cup - ball.spot);
        let direction = if ball.is_teed() && (ball.spot - tee.position).length() < 5.0 {
            flat(tee.aim)
        } else if to_cup.length() > REACH_M {
            play_line_aim(&tee.play_line, ball.spot).unwrap_or(to_cup)
        } else {
            to_cup
        };
        aim_direction(direction)
    }

    pub(super) fn previous_club(&self) -> Option<ClubId> {
        self.record
            .entries
            .iter()
            .rev()
            .find_map(|entry| match entry {
                Entry::Stroke(stroke) => Some(stroke.facts.club_id),
                Entry::Decision(_) => None,
            })
    }

    fn caddie_club(&self, world: &World<'_>, aim: Vec3, cup: Vec3, distance: Real) -> ClubId {
        let tee = self.record.setup.tee(&self.course).expect("checked tee");
        let fairway = BallLie::on(Surface::Fairway, Sit::Normal, tee.position);
        // The starting carry table is the Stock carry at the tee, even on later shots.
        let tee_aim = aim_direction(flat(tee.aim));
        let clubs = &bag::ids()[ClubId::Wood3 as usize..=ClubId::LobWedge as usize];
        let start = clubs
            .iter()
            .enumerate()
            .filter_map(|(index, &club)| {
                let input = nominal_input(world, fairway, club, Address::stock(club), tee_aim);
                let carry = stroke::preview(&input, world, None, None).carry_m;
                carry
                    .is_finite()
                    .then_some((index, (carry - distance).abs()))
            })
            .min_by(|left, right| left.1.total_cmp(&right.1));
        let Some((mut index, _)) = start else {
            return self.previous_club().unwrap_or(ClubId::Iron7);
        };
        let needed = distance * CARRY_SHARE;
        let carry_to_flag = |club| {
            let input = nominal_input(world, self.ball_lie, club, Address::stock(club), aim);
            stroke::preview(&input, world, Some(cup), None).carry_to_target_m
        };
        let carry = carry_to_flag(clubs[index]);
        if carry.is_nan() {
            return clubs[index];
        }
        let toward_shorter = carry >= needed;
        for _ in 0..SEARCH_STEPS {
            let Some(next) = index.checked_add_signed(if toward_shorter { 1 } else { -1 }) else {
                break;
            };
            let Some(&club) = clubs.get(next) else { break };
            let carry = carry_to_flag(club);
            if carry.is_nan() || (toward_shorter && carry < needed) {
                break;
            }
            index = next;
            if !toward_shorter && carry >= needed {
                break;
            }
        }
        clubs[index]
    }
}

fn flat(vector: Vec3) -> Vec3 {
    Vec3::new(vector.x, 0.0, vector.z)
}

fn aim_direction(direction: Vec3) -> Vec3 {
    if direction.length() > 1e-6 {
        direction.normalized()
    } else {
        Vec3::new(0.0, 0.0, -1.0)
    }
}

fn play_line_aim(line: &[Vec3], spot: Vec3) -> Option<Vec3> {
    let (mut index, _) = line.iter().enumerate().min_by(|(_, left), (_, right)| {
        flat(**left - spot)
            .length_squared()
            .total_cmp(&flat(**right - spot).length_squared())
    })?;
    let mut walked = 0.0;
    while index + 1 < line.len() && walked < REACH_M {
        walked += (line[index + 1] - line[index]).length();
        index += 1;
    }
    Some(flat(line[index] - spot))
}

fn nominal_input(
    world: &World<'_>,
    ball: BallLie,
    club: ClubId,
    address: Address,
    aim: Vec3,
) -> StrokeInput {
    let lie = world.address_lie(ball, &address.footprint(club), aim);
    StrokeInput::new(
        DeliveryInput {
            club,
            address,
            lie,
            attempt_seed: Seed(0),
        },
        aim,
    )
}

fn invalid(path: &str, message: &str) -> ActionRefusal {
    ActionRefusal::Invalid(
        Error::Malformed {
            path: path.into(),
            message: message.into(),
        }
        .into(),
    )
}
