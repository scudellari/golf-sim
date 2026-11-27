//! Build club delivery from a resolved address, lie, and swing.

use golf_sim_base::{
    ClubId, Real, Seed, Surface, fm,
    num::{clampf, deg_to_rad, lerpf},
    seed::SeedStream,
    trace::{Phase, Probe, Source},
};
use golf_sim_conditions::lies::AddressLie;
use golf_sim_physics::{
    facade::{sole_height_for_strike, standard_ball, standard_club},
    values::{ClubSpec, Delivery, Lie},
};
use golf_sim_swing::execution::Execution;

use crate::{address::Address, tuning as t};

#[derive(Clone, Debug, PartialEq)]
pub struct DeliveryInput {
    pub club: ClubId,
    pub address: Address,
    pub lie: AddressLie,
    pub attempt_seed: Seed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BuiltDelivery {
    pub club: ClubSpec,
    pub delivery: Delivery,
    pub lie: Lie,
    pub fault: Real,
}

impl DeliveryInput {
    pub fn on_a_peg(&self) -> bool {
        self.lie.ball.is_teed() && self.address.tee_height_m > 0.0
    }

    pub fn peg_share(&self) -> Real {
        if !self.on_a_peg() {
            return 0.0;
        }
        let stock_tee = Address::stock(self.club).tee_height_m;
        if stock_tee > 0.0 {
            clampf(self.address.tee_height_m / stock_tee, 0.0, 1.0)
        } else {
            1.0
        }
    }

    pub fn expected_speed_mps(&self, power: Real) -> Real {
        let speed_fraction = if power > 1.0 {
            1.0 + t::OVERSWING_SPEED_GAIN * (power - 1.0)
        } else if self.club == ClubId::Putter {
            power
        } else {
            lerpf(t::SPEED_AT_ZERO_POWER, 1.0, power)
        };
        let slopes = self.lie.slope_along_rad.abs() + self.lie.slope_across_rad.abs();
        let feet_in_sand = Real::from(u8::from(self.lie.lead_foot_surface == Surface::Bunker))
            + Real::from(u8::from(self.lie.trail_foot_surface == Surface::Bunker));
        t::club_speed_mps(self.club)
            * speed_fraction
            * (1.0 - t::GRIP_SPEED_PER_M * self.address.grip_down_m)
            * (1.0 - t::SLOPE_SPEED_LOSS_PER_RAD * slopes)
            * (1.0 - t::SAND_FOOT_SPEED_LOSS * feet_in_sand)
    }

    pub fn expected_toe_m(&self) -> Real {
        let bias = t::SLOPE_TOE_BIAS_M_PER_RAD * self.lie.slope_across_rad;
        if bias < 0.0 {
            (bias + t::GRIP_TOE_RELIEF_M_PER_M * self.address.grip_down_m).min(0.0)
        } else {
            bias
        }
    }
}

pub fn fault_curve(timing_fault: Real) -> Real {
    let magnitude = timing_fault.abs();
    (0.5 * magnitude.min(1.0) + 0.5 * fm::asinh((magnitude - 1.0).max(0.0))).copysign(timing_fault)
}

/// Previews disable hidden noise and spread. A timeout still resolves as a seeded mishit.
pub fn build<P: Probe>(
    input: &DeliveryInput,
    execution: Execution,
    with_noise: bool,
    probe: &mut P,
) -> BuiltDelivery {
    let club = standard_club(input.club);
    let row = t::club(input.club);
    let address = &input.address;
    let stock = Address::stock(input.club);
    let lie = input.lie.sim_lie(address.tee_height_m);
    let putter = input.club == ClubId::Putter;

    if P::ENABLED {
        probe.begin_phase(Phase::Delivery);
    }
    let (fault, power, timeout_toe, spread_u) = if execution.timed_out {
        let mut stream = SeedStream::derived(input.attempt_seed, "timeout");
        let sign = record_draw(probe, "timeout.sign", stream.next_sign());
        let size = record_draw(
            probe,
            "timeout.fault",
            stream.next_range(t::TIMEOUT_FAULT_MIN, t::TIMEOUT_FAULT_MAX),
        );
        let power_scale = record_draw(
            probe,
            "timeout.power",
            stream.next_range(t::TIMEOUT_POWER_MIN, 1.0),
        );
        let toe = record_draw(probe, "timeout.toe", stream.next_gaussian());
        (
            sign * size,
            execution.power * power_scale,
            toe * t::TIMEOUT_TOE_SD_M,
            t::SPREAD_CAP_U,
        )
    } else {
        (
            fault_curve(execution.timing_fault),
            execution.power,
            0.0,
            (execution.timing_fault.abs() - 1.0).clamp(0.0, t::SPREAD_CAP_U),
        )
    };
    let overshoot = (power - address.swing_length).max(0.0);
    let spread_u = (spread_u + t::OVERSHOOT_SPREAD_U * overshoot).min(t::SPREAD_CAP_U);
    let (hidden_scale, spread_scale) = if with_noise {
        let putt_scale = if putter { t::PUTT_HIDDEN_SCALE } else { 1.0 };
        (
            (1.0 + t::OVERSWING_NOISE_GAIN * overshoot) * putt_scale,
            spread_u * t::SPREAD_GAIN,
        )
    } else {
        (0.0, 0.0)
    };
    let (hidden, spread) = execution_noise(
        input.attempt_seed,
        putter,
        hidden_scale,
        spread_scale,
        probe,
    );
    let pattern = match (putter, fault > 0.0) {
        (true, true) => t::PUTT_LATE,
        (true, false) => t::PUTT_EARLY,
        (false, true) => t::LATE,
        (false, false) => t::EARLY,
    };
    let size = fault.abs();
    let speed = input.expected_speed_mps(power)
        * (1.0 + pattern.speed * size + hidden.speed + spread.speed);

    let arc = club.arc_radius_m;
    let reach = t::LOW_POINT_LIMIT * arc;
    let peg = input.peg_share();
    let peg_attack = deg_to_rad(row.attack_deg);
    let ground_attack = peg_attack.min(deg_to_rad(t::UNTEED_ATTACK_MAX_DEG));
    let attack0 = lerpf(ground_attack, peg_attack, peg);
    let loft0 =
        club.loft_rad + deg_to_rad(row.loft_delta_deg) + t::LOFT_PER_ARC * (attack0 - peg_attack);
    let low_point0 = -arc * fm::sin(attack0);
    let hands = address.hands_forward_rad - stock.hands_forward_rad;
    let low_point_setup = clampf(
        low_point0 - (address.ball_forward_m - stock.ball_forward_m)
            + hands * t::HANDS_LOW_POINT_M_PER_RAD,
        -reach,
        reach,
    );
    let loft_setup = loft0 + t::LOFT_PER_ARC * (-fm::asin(low_point_setup / arc) - attack0) - hands
        + t::FACE_OPEN_LOFT * address.face_open_rad;
    let low_point = clampf(
        low_point_setup + pattern.lp_m * size + hidden.lp_m + spread.lp_m,
        -reach,
        reach,
    );
    let attack = -fm::asin(low_point / arc);

    let strike = if execution.timed_out {
        None
    } else {
        execution.strike_m
    };
    let nominal = Delivery {
        club_speed_mps: speed,
        attack_rad: attack0,
        dynamic_loft_rad: loft0,
        ..Delivery::default()
    };
    let ball = standard_ball();
    let strike_up0 = row.strike_up_mm / 1000.0;
    let sole_for = |lie: &Lie, up| sole_height_for_strike(&club, &ball, &nominal, lie, up);
    let bunker = lie.surface == Surface::Bunker;
    let mut sole0 = sole_for(&lie, strike_up0).max(t::sole_clearance_m(lie.surface));
    if bunker {
        sole0 += lie.ball_height_m.min(0.0);
    }

    // In sand the arc stays under the stance. Moving the ball forward can turn a pick
    // into a splash, because the club reaches the sand well before it reaches the ball.
    let follows = if bunker { 0.0 } else { t::ARC_FOLLOWS_SETUP };
    let expected_rise = lerpf(
        arc_rise(arc, low_point0),
        arc_rise(arc, low_point_setup),
        follows,
    );
    let mut peg_strike = 0.0;
    if peg > 0.0 {
        let arc_lie = Lie::tee(
            stock.tee_height_m + t::TEE_ARC_FOLLOW * (address.tee_height_m - stock.tee_height_m),
        );
        let peg_sole0 = sole_for(&arc_lie, strike_up0);
        sole0 = lerpf(sole0, peg_sole0, peg);
        let strike_up = strike.map_or(0.0, |strike| strike.y) + hidden.up_m;
        peg_strike = peg * (sole_for(&arc_lie, strike_up0 + strike_up) - peg_sole0);
    }
    let sole = sole0 - expected_rise
        + pattern.depth_m * size
        + hidden.depth_m
        + arc_rise(arc, low_point)
        + peg_strike;

    let toe_bias = input.expected_toe_m();
    let toe = if execution.timed_out {
        toe_bias + timeout_toe
    } else {
        strike.map_or(toe_bias + pattern.toe_m * size, |strike| strike.x)
            + (hidden.toe_m + spread.toe_m)
    };
    let plane = deg_to_rad(row.plane_deg);
    let swing_direction = -Delivery::path_on_plane(0.0, plane, attack0) - address.stance_open_rad
        + deg_to_rad(pattern.path_deg * size + hidden.path_deg + spread.path_deg);
    let tilt = if putter {
        input.lie.ball_slope_along_rad
    } else {
        input.lie.slope_along_rad
    };
    let delivery = Delivery {
        club_speed_mps: speed.max(0.0),
        attack_rad: attack + tilt,
        dynamic_loft_rad: loft_setup + deg_to_rad(pattern.loft_deg * size + hidden.loft_deg) + tilt,
        path_rad: Delivery::path_on_plane(swing_direction, plane, attack),
        face_rad: address.face_open_rad
            + deg_to_rad(pattern.face_deg * size + hidden.face_deg + spread.face_deg),
        dynamic_lie_rad: t::SLOPE_LIE_FRACTION * input.lie.slope_across_rad,
        sole_height_m: sole,
        strike_toe_m: clampf(toe, -club.face_half_width_m, club.face_half_width_m),
    };

    trace_setup(probe, input.club, row, fault, low_point);
    BuiltDelivery {
        club,
        delivery,
        lie,
        fault,
    }
}

fn arc_rise(radius: Real, along: Real) -> Real {
    radius - fm::sqrt(radius * radius - along * along)
}

fn execution_noise(
    seed: Seed,
    putter: bool,
    hidden_scale: Real,
    spread_scale: Real,
    probe: &mut impl Probe,
) -> (t::Hidden, t::Spread) {
    let mut stream = SeedStream::derived(seed, "execution");
    let mut draw = |name, deviation, scale| {
        record_draw(probe, name, stream.next_gaussian()) * deviation * scale
    };

    // Draw every field in order, including ones that are disabled for this stroke.
    let h = t::HIDDEN;
    let hidden = t::Hidden {
        face_deg: draw("hidden.face_deg", h.face_deg, hidden_scale),
        path_deg: draw("hidden.path_deg", h.path_deg, hidden_scale),
        loft_deg: draw("hidden.loft_deg", h.loft_deg, hidden_scale),
        speed: draw("hidden.speed", h.speed, hidden_scale),
        lp_m: draw("hidden.lp_m", h.lp_m, hidden_scale),
        depth_m: draw("hidden.depth_m", h.depth_m, hidden_scale),
        toe_m: draw("hidden.toe_m", h.toe_m, hidden_scale),
        up_m: draw("hidden.up_m", h.up_m, hidden_scale),
    };
    let s = if putter { t::PUTT_SPREAD } else { t::SPREAD };
    let spread = t::Spread {
        face_deg: draw("spread.face_deg", s.face_deg, spread_scale),
        path_deg: draw("spread.path_deg", s.path_deg, spread_scale),
        speed: draw("spread.speed", s.speed, spread_scale),
        toe_m: draw("spread.toe_m", s.toe_m, spread_scale),
        lp_m: draw("spread.lp_m", s.lp_m, spread_scale),
    };
    (hidden, spread)
}

fn record_draw<P: Probe>(probe: &mut P, name: &str, draw: Real) -> Real {
    if P::ENABLED {
        probe.value(name, draw, "1", Source::SeedDraw { name: name.into() });
    }
    draw
}

fn trace_setup<P: Probe>(
    probe: &mut P,
    club: ClubId,
    row: t::ClubTuning,
    fault: Real,
    low_point: Real,
) {
    if P::ENABLED {
        for (name, value, unit) in [
            ("club_speed_mps", t::club_speed_mps(club), "m/s"),
            ("attack_deg", row.attack_deg, "deg"),
            ("loft_delta_deg", row.loft_delta_deg, "deg"),
            ("plane_deg", row.plane_deg, "deg"),
            ("strike_up_mm", row.strike_up_mm, "mm"),
        ] {
            probe.value(name, value, unit, Source::Tunable { name: name.into() });
        }
        probe.value("fault", fault, "1", Source::Computed);
        probe.value("low_point", low_point, "m", Source::Computed);
    }
}
