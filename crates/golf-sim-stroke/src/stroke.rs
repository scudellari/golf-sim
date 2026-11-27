//! Play and preview a resolved stroke in a fixed world.

use golf_sim_base::{
    ClubId, Real, Seed, Surface, Vec2, Vec3,
    num::lerpf,
    trace::{NoProbe, Probe},
};
use golf_sim_conditions::{lies::BallLie, world::World};
use golf_sim_course::legal::StanceFootprint;
use golf_sim_physics::{
    facade as sim,
    plane::{ConstantAir, PlaneGround},
    samplers::{AirSampler, GroundSample, GroundSampler},
    values::{EndReason, Launch, ShotRequest, ShotResult},
};
use golf_sim_swing::spec::SwingSpec;

use crate::{
    address::{Address, Assists, Intent, Setting},
    delivery::{self, DeliveryExecution, DeliveryInput},
    results::{Aloft, DropResult, Preview, StrokeOutcome, StrokeResult},
    tuning as t,
    wrappers::{self, FlightAir},
};

#[derive(Clone, Debug, PartialEq)]
pub struct StrokeInput {
    pub delivery: DeliveryInput,
    /// Horizontal direction of the target line.
    pub aim: Vec3,
    /// Seconds from address to impact, used to pick the gust.
    pub gust_time_s: Real,
    pub flagstick_in: bool,
}

impl StrokeInput {
    pub fn new(delivery: DeliveryInput, aim: Vec3) -> Self {
        let flagstick_in = delivery.lie.ball.surface != Surface::Green;
        Self {
            delivery,
            aim,
            gust_time_s: 0.0,
            flagstick_in,
        }
    }

    pub fn ball_position(&self) -> Vec3 {
        self.delivery.lie.ball.spot
    }
}

pub fn stock(club: ClubId) -> Address {
    Address::stock(club)
}

pub fn from_intent(club: ClubId, intent: Intent, base: Option<&Address>) -> Address {
    Address::from_intent(club, intent, base)
}

pub fn adjust(club: ClubId, address: Address, setting: Setting, steps: i32) -> Address {
    address.adjust(club, setting, steps)
}

pub fn footprint(club: ClubId, address: &Address) -> StanceFootprint {
    address.footprint(club)
}

pub fn swing_spec(input: &StrokeInput, assists: Assists) -> SwingSpec {
    let delivery = &input.delivery;
    let lie = &delivery.lie;
    let address = &delivery.address;
    let slopes = 1.0
        + t::SLOPE_ACROSS_DIFFICULTY_PER_RAD * lie.slope_across_rad.abs()
        + t::SLOPE_ALONG_DIFFICULTY_PER_RAD * lie.slope_along_rad.abs();
    let length = lerpf(t::SHORT_SWING_DIFFICULTY, 1.0, address.swing_length);
    let grip = 1.0 + t::GRIP_WINDOW_PER_M * address.grip_down_m;
    let difficulty = t::club(delivery.club).difficulty
        * t::surface_difficulty(lie.ball.surface)
        * slopes
        * length
        / grip;
    let window = t::BASE_WINDOW_S / difficulty * assists.window_scale;
    let lean = (lie.slope_along_rad * t::SLOPE_FAULT_BIAS_PER_RAD)
        .clamp(-t::FAULT_BIAS_LIMIT, t::FAULT_BIAS_LIMIT);
    let drift = if delivery.club == ClubId::Putter {
        t::PUTT_DRIFT_SCALE
    } else {
        1.0
    };
    let vertical = delivery.peg_share();
    let scale_drift = |value: Vec2| Vec2::new(value.x, value.y * vertical) * drift;
    SwingSpec {
        target_power: address.swing_length,
        meter_rate_per_s: t::METER_RATE_PER_S,
        ring_min_s: t::RING_MIN_S,
        ring_max_s: t::RING_MAX_S,
        window_early_s: (window * (1.0 + lean)).max(t::MIN_WINDOW_S),
        window_late_s: (window * (1.0 - lean)).max(t::MIN_WINDOW_S),
        timeout_after_s: t::TIMEOUT_AFTER_S,
        strike_mode: assists.strike_mode,
        drift_bias_m: Vec2::new(delivery.expected_toe_m(), 0.0),
        drift_start_sd_m: scale_drift(t::DRIFT_START_SD_M),
        drift_slide_sd_mps: scale_drift(t::DRIFT_SLIDE_SD_MPS),
        steer_vertical: delivery.on_a_peg(),
        wobble_m: scale_drift(t::DRIFT_WOBBLE_M),
        ..SwingSpec::default()
    }
}

pub fn reticle_seed(attempt_seed: Seed) -> Seed {
    attempt_seed.derive("reticle")
}

pub fn drop(
    spot: Vec3,
    knee_height_m: Real,
    world: &World<'_>,
    probe: &mut impl Probe,
) -> DropResult {
    let ground = world.ground();
    let ball = sim::standard_ball();
    let mut result = DropResult {
        first: None,
        rest: None,
    };
    let mut release = spot;
    // On a slope the contact point is offset horizontally from the ball's centre.
    for _ in 0..4 {
        release.y = ground.sample_ground(release.x, release.z).height_m + knee_height_m;
        let shot = sim::simulate_drop(
            &ball,
            release,
            world.clock_s(),
            &world.air_mean(release),
            ground,
            false,
            probe,
        );
        let Some(touch) = shot.touchdowns.first() else {
            break;
        };
        result.first = Some(touch.contact_point);
        result.rest = (shot.end_reason == EndReason::Rest).then_some(shot.rest_position);
        let offset = spot - touch.contact_point;
        let correction = Vec3::new(offset.x, 0.0, offset.z);
        if correction.length() <= 0.00025 {
            break;
        }
        release += correction;
    }
    result
}

pub fn place(spot: Vec3, world: &World<'_>, seed: Seed, probe: &mut impl Probe) -> BallLie {
    world.settle(spot, seed, probe)
}

pub fn play(
    input: &StrokeInput,
    execution: DeliveryExecution,
    world: &World<'_>,
    probe: &mut impl Probe,
) -> StrokeResult {
    let (mut request, fault) = request(input, execution, true, probe);
    request.time_s = world.clock_s() + input.gust_time_s;
    request.cup = world.cup(input.flagstick_in);
    let air = world.air_at_impact(input.gust_time_s);
    let ground = world.ground();
    let playback = sim::simulate_shot(&request, air, ground, true, probe);
    let launch = playback
        .launch
        .as_ref()
        .expect("a simulated shot records its launch");
    let fat = wrappers::fat(&request, launch, air, ground);
    StrokeResult {
        outcome: StrokeOutcome::from_shot(&playback, &request),
        quality: wrappers::quality(input.delivery.club, execution, launch, fat),
        playback,
        fault,
        fat,
    }
}

pub fn launch(
    input: &StrokeInput,
    execution: DeliveryExecution,
    world: &World<'_>,
    probe: &mut impl Probe,
) -> Launch {
    let (mut request, _) = request(input, execution, true, probe);
    request.time_s = world.clock_s() + input.gust_time_s;
    sim::strike(
        &request,
        world.air_at_impact(input.gust_time_s),
        world.ground(),
        probe,
    )
}

pub fn nominal_execution(input: &StrokeInput) -> DeliveryExecution {
    DeliveryExecution {
        power: input.delivery.address.swing_length,
        strike_m: Some(Vec2::ZERO),
        ..DeliveryExecution::default()
    }
}

/// The nominal shot in the world's mean air, with noise and playback disabled.
pub fn nominal_shot(input: &StrokeInput, world: &World<'_>) -> ShotResult {
    let request = nominal_request(input, world);
    sim::simulate_shot(
        &request,
        &world.air_mean(request.position),
        world.ground(),
        false,
        &mut NoProbe,
    )
}

/// Carry in standard air on level ground, independent of today's conditions. NaN for putts.
pub fn stock_carry(input: &StrokeInput) -> Real {
    if input.delivery.club == ClubId::Putter {
        Real::NAN
    } else {
        let spot = input.ball_position();
        nominal_flight(
            input,
            &ConstantAir::new(sim::standard_air()),
            &level_at(spot),
        )
        .carry_m()
    }
}

/// Preview the nominal swing in mean air. NaN marks an unavailable distance.
pub fn preview(
    input: &StrokeInput,
    world: &World<'_>,
    target: Option<Vec3>,
    stock_address: Option<&Address>,
) -> Preview {
    let mut preview = Preview::default();
    let spot = input.ball_position();
    let ground = world.ground();
    let here = world.air_mean(spot);
    let standard = ConstantAir::new(sim::standard_air());
    let request = nominal_request(input, world);
    let launch = sim::strike(&request, &here, ground, &mut NoProbe);
    let putt = input.delivery.club == ClubId::Putter;
    let ball = &request.ball;
    let time = request.time_s;
    if !putt {
        preview.carry_m = sim::carry_to_height(&launch, ball, time, &here, ground, spot.y);
    }
    if preview.carry_m.is_nan() {
        let roll = sim::simulate_launch(
            &launch,
            ball,
            time,
            &here,
            ground,
            request.cup,
            putt,
            &mut NoProbe,
        );
        preview.roll_m = roll.total_m();
        if let Some(&start) = roll.positions.first() {
            for position in roll.positions {
                preview.putt_line.push(position);
                if (position - start).length() >= 0.9 {
                    break;
                }
            }
        }
    } else if let Some(target) = target.filter(|target| target.is_finite()) {
        let offset = target - spot;
        preview.target_distance_m = Vec2::new(offset.x, offset.z).length();
        let met = FlightAir::new(&here);
        let height = ground.sample_ground(target.x, target.z).height_m;
        let carry_here = sim::carry_to_height(&launch, ball, time, &met, ground, height);
        preview.carry_to_target_m = carry_here;
        let standard_launch = sim::strike(&request, &standard, ground, &mut NoProbe);
        let carry_standard =
            sim::carry_to_height(&standard_launch, ball, time, &standard, ground, spot.y);
        if carry_here > 0.0 && !carry_standard.is_nan() {
            preview.plays_like_m = preview.target_distance_m * carry_standard / carry_here;
            preview.wind_aloft_mps = met.mean_wind_mps();
            preview.wind_here_mps = here.sample_air(time, spot).mean_wind_mps;
            let change = preview.wind_aloft_mps.length() - preview.wind_here_mps.length();
            if change.abs() >= t::WIND_ALOFT_NOTICEABLE_MPS {
                preview.aloft = if change > 0.0 {
                    Aloft::More
                } else {
                    Aloft::Less
                };
            }
        }
    }

    let baseline = stock_address
        .cloned()
        .unwrap_or_else(|| stock(input.delivery.club));
    if !input.delivery.address.same_as(&baseline) {
        let level = level_at(spot);
        let mine = nominal_flight(input, &standard, &level);
        let mut stock_input = input.clone();
        stock_input.delivery.address = baseline;
        let theirs = nominal_flight(&stock_input, &standard, &level);
        preview.has_hint = true;
        preview.hint_carry_m = mine.carry_m() - theirs.carry_m();
        preview.hint_apex_m = mine.apex_height_m() - theirs.apex_height_m();
        preview.hint_offline_m = mine.total_side_m() - theirs.total_side_m();
    }
    preview
}

fn level_at(spot: Vec3) -> PlaneGround {
    PlaneGround::new(GroundSample::default(), spot)
}

fn nominal_flight(input: &StrokeInput, air: &ConstantAir, ground: &PlaneGround) -> ShotResult {
    let (request, _) = request(input, nominal_execution(input), false, &mut NoProbe);
    sim::simulate_shot(&request, air, ground, false, &mut NoProbe)
}

fn nominal_request(input: &StrokeInput, world: &World<'_>) -> ShotRequest {
    let (mut request, _) = request(input, nominal_execution(input), false, &mut NoProbe);
    request.time_s = world.clock_s();
    request.cup = world.cup(input.flagstick_in);
    request
}

fn request(
    input: &StrokeInput,
    execution: DeliveryExecution,
    with_noise: bool,
    probe: &mut impl Probe,
) -> (ShotRequest, Real) {
    let built = delivery::build(&input.delivery, execution, with_noise, probe);
    let mut request = ShotRequest::new(built.club, sim::standard_ball(), built.delivery, built.lie);
    request.position = input.ball_position();
    request.aim = input.aim;
    (request, built.fault)
}
