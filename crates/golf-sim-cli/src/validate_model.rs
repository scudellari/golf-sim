//! Simulated values for comparison with measured targets.

use std::{collections::BTreeMap, path::Path};

use golf_sim::{
    base::{ClubId, Real, Seed, Surface, Vec2, Vec3, fm, trace::NoProbe},
    conditions::{
        grounds::PracticeGround,
        lies::{BallLie, Sit},
        world::World,
    },
    physics::{
        bounce, cup, facade as sim, flight,
        plane::{ConstantAir, PlaneGround},
        samplers::{AirSample, GroundSample, GroundSampler},
        tunables as t,
        values::{
            ClubSpec, Cup, Delivery, EndReason, EventKind, Launch, LaunchReadout, Lie, ShotRequest,
            ShotResult, units,
        },
    },
    stroke::{
        delivery::DeliveryInput,
        stroke::{self, StrokeInput},
    },
};
use golf_sim_validate::{
    Conditions, Corpus, Kind, LoadError, Model, Quantities, Quantity, Report, Status, Target,
};

const FORWARD: Vec3 = Vec3::new(0.0, 0.0, -1.0);

pub fn report(data: Option<&Path>) -> Result<Report, LoadError> {
    let corpus = Corpus::load_default(data)?;
    Ok(corpus.run(&Simulation { corpus: &corpus }))
}

pub fn failures(report: &Report) -> impl Iterator<Item = String> + '_ {
    report.targets.iter().flat_map(|target| {
        target.quantities.iter()
            .filter(|(_, quantity)| matches!(quantity.status, Status::Failed | Status::NotRun))
            .map(|(name, quantity)| {
                format!(
                    "{} {name} ({}): predicted {:?}, measured {}, residual {:?}, allowed earlier residual {:?}",
                    target.id, quantity.status, quantity.predicted, quantity.measured,
                    quantity.residual, quantity.allowed_earlier_residual,
                )
            })
    })
}

type OperandMagnitudes = BTreeMap<String, Real>;

struct Simulation<'a> {
    corpus: &'a Corpus,
}

impl Model for Simulation<'_> {
    fn predict(&self, target: &Target, conditions: &Conditions) -> Option<Quantities> {
        self.predict_with_operand_magnitudes(target, conditions)
            .map(|(quantities, _)| quantities)
    }

    fn predict_with_operand_magnitudes(
        &self,
        target: &Target,
        conditions: &Conditions,
    ) -> Option<(Quantities, OperandMagnitudes)> {
        let inputs = Inputs(target);
        let air = air(inputs, conditions);
        let ground = ground(conditions)?;
        let mut operands = OperandMagnitudes::new();
        let mut predictions = match target.kind {
            Kind::Aero => {
                let (cd, cl) =
                    flight::coefficients(inputs.number("reynolds")?, inputs.number("spin_ratio")?);
                numbers([("cd", cd), ("cl", cl)])
            }
            Kind::Launch | Kind::RunOut => {
                let shot = launch_shot(inputs, &air, &ground)?;
                let carry = units::to_yards(shot.carry_m());
                let mut predictions = numbers([
                    ("carry_yd", carry),
                    ("apex_yd", units::to_yards(shot.apex_height_m())),
                    ("descent_deg", shot.land_angle_rad().to_degrees()),
                    ("run_yd", units::to_yards(shot.run_m())),
                ]);
                if target.measured.contains_key("carry_gain_yd") {
                    let baseline = launch_shot(
                        Inputs(self.corpus.carry_gain_baseline(target)?),
                        &air,
                        &ground,
                    )?;
                    difference(
                        &mut predictions,
                        &mut operands,
                        "carry_gain_yd",
                        carry,
                        units::to_yards(baseline.carry_m()),
                    );
                }
                if target.measured.contains_key("carry_change_yd") {
                    let mut calm = air;
                    calm.air.mean_wind_mps = Vec3::ZERO;
                    let baseline = launch_shot(inputs, &calm, &ground)?;
                    difference(
                        &mut predictions,
                        &mut operands,
                        "carry_change_yd",
                        carry,
                        units::to_yards(baseline.carry_m()),
                    );
                }
                predictions
            }
            Kind::Delivery => delivery(inputs, conditions, air, ground, &mut operands)?,
            Kind::TurfImpact => {
                // The impact impulse can nearly cancel the incoming spin.
                operands.insert("out_spin_rad_s".into(), inputs.number("spin_rad_s")?.abs());
                turf(inputs, conditions, &air, &ground)?
            }
            Kind::GreenSpeed => green_speed(inputs, conditions, &air)?,
            Kind::CupCapture => capture(inputs, conditions, &air)?,
            Kind::StockCarry => stock(inputs, air, ground)?,
        };
        predictions.retain(|name, _| target.measured.contains_key(name));
        Some((predictions, operands))
    }
}

#[derive(Clone, Copy)]
pub struct Inputs<'a>(pub &'a Target);

impl<'a> Inputs<'a> {
    pub fn number(self, name: &str) -> Option<Real> {
        self.0.inputs.get(name)?.as_number()
    }

    fn text(self, name: &str) -> Option<&'a str> {
        self.0.inputs.get(name)?.as_str()
    }

    pub fn club(self) -> Option<ClubId> {
        ClubId::from_name(self.text("club")?)
    }
}

fn numbers<const N: usize>(values: [(&str, Real); N]) -> Quantities {
    values
        .into_iter()
        .map(|(key, value)| (key.to_owned(), Quantity::Number(value)))
        .collect()
}

fn difference(
    predictions: &mut Quantities,
    operands: &mut OperandMagnitudes,
    name: &str,
    value: Real,
    baseline: Real,
) {
    predictions.insert(name.to_owned(), Quantity::Number(value - baseline));
    operands.insert(name.to_owned(), value.abs() + baseline.abs());
}

pub fn air(inputs: Inputs<'_>, conditions: &Conditions) -> ConstantAir {
    let mut sample = AirSample {
        temperature_c: conditions.air_temp_c,
        pressure_pa: conditions.pressure_pa,
        relative_humidity: conditions.humidity,
        ..AirSample::default()
    };
    let wind = inputs.number("wind_mph").unwrap_or(conditions.wind_mph);
    if wind != 0.0 {
        // These targets specify wind uniform with height.
        sample.mean_wind_mps.z = units::mph(wind);
        sample.roughness_length_m = 1e-6;
        sample.wind_reference_height_m = 15.0;
    }
    ConstantAir::new(sample)
}

pub fn ground(conditions: &Conditions) -> Option<PlaneGround> {
    Some(PlaneGround::new(
        GroundSample {
            surface: Surface::from_name(&conditions.ground.surface.to_ascii_uppercase())?,
            firmness: conditions.ground.firmness,
            moisture: conditions.ground.moisture,
            ..GroundSample::default()
        },
        Vec3::ZERO,
    ))
}

fn green(conditions: &Conditions, stimp_ft: Real, grade: Real) -> PlaneGround {
    let sample = GroundSample {
        surface: Surface::Green,
        firmness: conditions.green.firmness,
        moisture: conditions.green.moisture,
        green_speed_m: units::feet(stimp_ft),
        ..GroundSample::default()
    };
    PlaneGround::sloped(sample, FORWARD, grade)
}

fn launch(at: Vec3, aim: Vec3, speed: Real, angle_deg: Real, spin: Real) -> Launch {
    Launch::from_readout(
        &t::BALL,
        at,
        aim,
        LaunchReadout {
            ball_speed_mps: speed,
            launch_angle_rad: angle_deg.to_radians(),
            spin_rate_rad_s: spin,
            ..LaunchReadout::default()
        },
    )
}

fn simulate(
    launch: &Launch,
    air: &ConstantAir,
    ground: &PlaneGround,
    cup: Option<Cup>,
) -> ShotResult {
    sim::simulate_launch(launch, &t::BALL, 0.0, air, ground, cup, false, &mut NoProbe)
}

pub fn launch_shot(
    inputs: Inputs<'_>,
    air: &ConstantAir,
    ground: &PlaneGround,
) -> Option<ShotResult> {
    let launch = launch(
        Vec3::ZERO,
        FORWARD,
        units::mph(inputs.number("ball_mph")?),
        inputs.number("launch_deg")?,
        units::rpm(inputs.number("spin_rpm")?),
    );
    let shot = simulate(&launch, air, ground, None);
    (!matches!(shot.end_reason, EndReason::InvalidInput)).then_some(shot)
}

fn turf(
    inputs: Inputs<'_>,
    conditions: &Conditions,
    air: &ConstantAir,
    ground: &PlaneGround,
) -> Option<Quantities> {
    if let Some(vx) = inputs.number("vx_mps") {
        // Both campaigns use the same fairway parameters for the baseline fit.
        let sample = ground.sample_ground(0.0, 0.0);
        let out = bounce::impact(
            Vec3::new(vx, inputs.number("vy_mps")?, 0.0),
            Vec3::new(0.0, 0.0, inputs.number("spin_rad_s")?),
            &sample,
            &t::BALL,
            &mut NoProbe,
        );
        return Some(numbers([
            ("out_vx_mps", out.velocity.x),
            ("out_vy_mps", out.velocity.y),
            ("out_spin_rad_s", out.spin.z),
        ]));
    }
    let green = green(conditions, conditions.green.stimp_ft, 0.0);
    let ground = if inputs.text("surface") == Some("green") {
        // The unclassified Haake rows use the nominal green and the same
        // short launch into the turf as their earlier predictions.
        if !inputs.0.id.starts_with("H.green") || inputs.text("green_class").is_some() {
            return green_impact(inputs, green.sample_ground(0.0, 0.0));
        }
        &green
    } else {
        ground
    };
    let incoming = launch(
        Vec3::new(0.0, 0.01, 0.0),
        FORWARD,
        inputs.number("speed_mps")?,
        -inputs.number("angle_deg")?,
        inputs.number("spin_rad_s")?,
    );
    let shot = simulate(&incoming, air, ground, None);
    let contact = shot
        .first_event(EventKind::Bounce)
        .or_else(|| shot.first_event(EventKind::RollStart))?;
    Some(numbers([
        ("out_speed_mps", contact.velocity.length()),
        (
            "out_angle_deg",
            fm::atan2(contact.velocity.y, horizontal(contact.velocity)).to_degrees(),
        ),
        ("out_spin_rad_s", contact.spin.x),
    ]))
}

fn green_impact(inputs: Inputs<'_>, mut sample: GroundSample) -> Option<Quantities> {
    use bounce::GreenClass::{Rolling, Slipping};

    let class = match inputs.text("green_class") {
        Some("rolling") => Rolling,
        Some("slipping") | None => Slipping,
        Some(_) => return None,
    };
    if let Some(stiffness) = inputs.number("stiffness_mpa") {
        sample.firmness = bounce::green_firmness(stiffness);
    } else if inputs.0.id.starts_with("SPIN07.") {
        // Nominal firmness for the championship greens and the free-draining Ganton green.
        sample.firmness = 1.0;
    } else if inputs.0.id.starts_with("H.ganton.") {
        sample.firmness = 0.8;
    }
    let speed = inputs.number("speed_mps")?;
    let angle = inputs.number("angle_deg")?.to_radians();
    let spin = inputs.number("spin_rad_s")?;
    let velocity =
        FORWARD * (speed * fm::cos(angle)) + Vec3::new(0.0, -speed * fm::sin(angle), 0.0);
    let out = bounce::impact_with_green_class(
        velocity,
        Vec3::new(spin, 0.0, 0.0),
        &sample,
        &t::BALL,
        class,
        &mut NoProbe,
    );
    // R04 includes rebounds that travel back toward the tee.
    let along = out.velocity.dot(FORWARD);
    Some(numbers([
        ("out_speed_mps", out.velocity.length()),
        (
            "out_angle_deg",
            fm::atan2(out.velocity.y, along).to_degrees(),
        ),
        ("out_spin_rad_s", out.spin.x),
    ]))
}

fn horizontal(vector: Vec3) -> Real {
    Vec2::new(vector.x, vector.z).length()
}

fn roll(
    speed: Real,
    aim: Vec3,
    air: &ConstantAir,
    ground: &PlaneGround,
    cup: Option<Cup>,
) -> ShotResult {
    let at = Vec3::new(0.0, ground.height_at(0.0, 0.0), 0.0);
    let launch = launch(at, aim, speed, 0.0, -speed / t::BALL.radius_m);
    simulate(&launch, air, ground, cup)
}

fn green_speed(
    inputs: Inputs<'_>,
    conditions: &Conditions,
    air: &ConstantAir,
) -> Option<Quantities> {
    let ground = green(
        conditions,
        inputs.number("stimp_ft")?,
        inputs.number("grade").unwrap_or_default(),
    );
    let speed = inputs.number("exit_mps")?;
    let up = roll(speed, FORWARD, air, &ground, None).total_m();
    Some(if inputs.number("grade").is_some() {
        let down = roll(speed, -FORWARD, air, &ground, None).total_m();
        numbers([("brede_ft", units::to_feet(2.0 * up * down / (up + down)))])
    } else {
        numbers([("roll_ft", units::to_feet(up))])
    })
}

fn capture(inputs: Inputs<'_>, conditions: &Conditions, air: &ConstantAir) -> Option<Quantities> {
    if inputs.0.measured.contains_key("holed_fraction") {
        let (offset, stick) =
            cup::PuttFlagstick::trial_cell(inputs.text("line")?, inputs.text("flagstick")?)?;
        let ground = green(conditions, 10.0, 0.0);
        let fraction = cup::replay_putts(
            &ground,
            air,
            units::feet(inputs.number("past_ft")?),
            offset,
            stick,
        )?;
        return Some(numbers([("holed_fraction", fraction)]));
    }
    let radius = t::CUP_RADIUS_M;
    let offset = inputs.number("offset_radii")? * radius;
    if inputs.0.measured.contains_key("capture_mps") {
        let grade = fm::sin(inputs.number("grade_deg").unwrap_or(0.0).to_radians());
        let uphill_sine = match inputs.text("direction") {
            Some("uphill") => grade,
            Some("downhill") => -grade,
            _ => 0.0,
        };
        let speed = cup::putt_capture_speed_on_slope(offset, uphill_sine);
        return Some(numbers([("capture_mps", speed)]));
    }
    let stimp = conditions.green.stimp_ft;
    let rim = 2.0 - fm::sqrt(radius * radius - offset * offset);
    let exit = inputs.number("exit_mps")?;
    let entry = inputs.number("entry_mps")?;
    let speed = fm::sqrt(entry * entry + exit * exit * rim / units::feet(stimp));
    let cup = Cup {
        position: Vec3::new(offset, 0.0, -2.0),
        flagstick_in: false,
    };
    let shot = roll(
        speed,
        FORWARD,
        air,
        &green(conditions, stimp, 0.0),
        Some(cup),
    );
    let entry = shot
        .first_event(EventKind::CupEntry)
        .map_or(0.0, |event| event.velocity.length());
    let mut predictions = numbers([("entry_mps", entry)]);
    predictions.insert(
        "holed".into(),
        Quantity::Bool(shot.end_reason == EndReason::Holed),
    );
    Some(predictions)
}

pub struct DeliveryProbe<'a> {
    inputs: Inputs<'a>,
    pub air: ConstantAir,
    pub ground: PlaneGround,
    lie: Lie,
}

impl<'a> DeliveryProbe<'a> {
    pub fn new(
        inputs: Inputs<'a>,
        conditions: &Conditions,
        air: ConstantAir,
        mut ground: PlaneGround,
    ) -> Option<DeliveryProbe<'a>> {
        let lie = match (inputs.club()?, inputs.text("lie")) {
            (_, Some("bunker")) => {
                let mut green = green(conditions, conditions.green.stimp_ft, 0.0);
                green.add_patch(
                    Vec2::new(-3.0, -1.0),
                    Vec2::new(3.0, 3.0),
                    GroundSample {
                        surface: Surface::Bunker,
                        ..GroundSample::default()
                    },
                );
                ground = green;
                Lie {
                    surface: Surface::Bunker,
                    sand_firmness: inputs.number("sand_firmness")?,
                    ..Lie::default()
                }
            }
            (ClubId::Driver, None) => Lie::tee(0.035),
            (ClubId::Putter, None) => {
                ground = green(conditions, conditions.green.stimp_ft, 0.0);
                Lie {
                    surface: Surface::Green,
                    ..Lie::default()
                }
            }
            (_, None) => Lie {
                ball_height_m: 0.005,
                ..Lie::default()
            },
            _ => return None,
        };
        Some(DeliveryProbe {
            inputs,
            air,
            ground,
            lie,
        })
    }

    pub fn from_target(target: &'a Target, conditions: &Conditions) -> Option<DeliveryProbe<'a>> {
        let inputs = Inputs(target);
        Self::new(
            inputs,
            conditions,
            air(inputs, conditions),
            ground(conditions)?,
        )
    }

    pub fn request(&self, club: &ClubSpec, loft: Real, toe: Real, up: Real) -> Option<ShotRequest> {
        let input = self.inputs;
        let mut delivery = Delivery {
            club_speed_mps: units::mph(input.number("club_mph")?),
            attack_rad: input.number("attack_deg")?.to_radians(),
            dynamic_loft_rad: loft.to_radians(),
            dynamic_lie_rad: input
                .number("dynamic_lie_deg")
                .unwrap_or_default()
                .to_radians(),
            face_rad: input.number("face_deg").unwrap_or_default().to_radians(),
            path_rad: input.number("path_deg").unwrap_or_default().to_radians(),
            strike_toe_m: toe,
            ..Delivery::default()
        };
        if let Some(sole_mm) = input.number("sole_height_mm") {
            delivery.sole_height_m = sole_mm * 0.001;
        } else if self.lie.surface == Surface::Tee || input.number("up_mm").is_some() {
            delivery.sole_height_m =
                sim::sole_height_for_strike(club, &t::BALL, &delivery, &self.lie, up);
        }
        Some(ShotRequest::new(club.clone(), t::BALL, delivery, self.lie))
    }

    pub fn strike(&self, club: &ClubSpec, loft: Real, toe: Real, up: Real) -> Option<Launch> {
        let launch = sim::strike(
            &self.request(club, loft, toe, up)?,
            &self.air,
            &self.ground,
            &mut NoProbe,
        );
        launch.valid.then_some(launch)
    }

    pub fn loft_for_launch(
        &self,
        club: &ClubSpec,
        toe: Real,
        up: Real,
        [mut low, mut high]: [Real; 2],
        steps: usize,
    ) -> Option<(Real, Launch)> {
        let launch_deg = self.inputs.number("launch_deg")?;
        let mut loft = (low + high) * 0.5;
        let mut launch = self.strike(club, loft, toe, up)?;
        for _ in 1..steps {
            if launch.readout.launch_angle_rad.to_degrees() < launch_deg {
                low = loft;
            } else {
                high = loft;
            }
            loft = (low + high) * 0.5;
            launch = self.strike(club, loft, toe, up)?;
        }
        Some((loft, launch))
    }
}

fn delivery(
    inputs: Inputs<'_>,
    conditions: &Conditions,
    air: ConstantAir,
    ground: PlaneGround,
    operands: &mut OperandMagnitudes,
) -> Option<Quantities> {
    let club_id = inputs.club()?;
    let club = sim::standard_club(club_id);
    let probe = DeliveryProbe::new(inputs, conditions, air, ground)?;
    let toe = inputs.number("toe_mm").unwrap_or_default() * 0.001;
    let up = inputs.number("up_mm").unwrap_or_default() * 0.001;
    let loft = match inputs.number("dynamic_loft_deg") {
        Some(loft) => loft,
        None => probe.loft_for_launch(&club, toe, up, [0.0, 50.0], 31)?.0,
    };
    let launch = probe.strike(&club, loft, toe, up)?;
    let values = readout(launch.readout);
    let mut predictions = numbers(values);
    let measured = &inputs.0.measured;
    predictions.extend(numbers([
        ("dynamic_loft_deg", loft),
        ("speed_ratio", launch.smash_factor),
    ]));
    if measured.keys().any(|name| name.ends_with("_change")) {
        let centre = probe.strike(&club, loft, 0.0, 0.0)?;
        for ((name, value), (_, baseline)) in values.into_iter().zip(readout(centre.readout)) {
            difference(
                &mut predictions,
                operands,
                &format!("{name}_change"),
                value,
                baseline,
            );
        }
    }
    let start = launch.readout.launch_direction_rad.to_degrees();
    if measured.contains_key("face_share") {
        let path = inputs.number("path_deg").unwrap_or_default();
        let face_to_path = inputs.number("face_deg")? - path;
        predictions.extend(numbers([("face_share", (start - path) / face_to_path)]));
    }
    if ["carry_m", "carry_yd", "curve_yd", "true_roll_in"]
        .iter()
        .any(|name| measured.contains_key(*name))
    {
        let shot = simulate(&launch, &probe.air, &probe.ground, None);
        let carry = shot.carry_m();
        let carry_yd = units::to_yards(carry);
        predictions.extend(numbers([("carry_m", carry), ("carry_yd", carry_yd)]));
        if measured.contains_key("curve_yd") {
            let curve = units::to_yards(
                shot.carry_side_m() - carry * fm::tan(launch.readout.launch_direction_rad),
            );
            let curve = conditions.normalize_curve(inputs.0, curve, carry_yd);
            predictions.extend(numbers([("curve_yd", curve)]));
        }
        if measured.contains_key("true_roll_in") {
            let position = shot.first_event(EventKind::PureRoll)?.position;
            predictions.extend(numbers([(
                "true_roll_in",
                horizontal(position) / units::INCH,
            )]));
        }
    }
    Some(predictions)
}

fn readout(launch: LaunchReadout) -> [(&'static str, Real); 5] {
    [
        ("ball_mph", units::to_mph(launch.ball_speed_mps)),
        ("launch_deg", launch.launch_angle_rad.to_degrees()),
        ("spin_rpm", units::to_rpm(launch.spin_rate_rad_s)),
        ("spin_axis_deg", launch.spin_axis_rad.to_degrees()),
        ("start_deg", launch.launch_direction_rad.to_degrees()),
    ]
}

fn stock(inputs: Inputs<'_>, mut air: ConstantAir, ground: PlaneGround) -> Option<Quantities> {
    let club = inputs.club()?;
    air.air.temperature_c = inputs.number("air_temp_c")?;
    air.air.pressure_pa = sim::pressure_at_altitude(inputs.number("altitude_m")?);
    let world = World::practice(
        PracticeGround::new(ground),
        air,
        BallLie::on(Surface::Fairway, Sit::Normal, Vec3::ZERO),
        None,
        0.0,
    );
    let address = stroke::stock(club);
    let ball = stroke::place(Vec3::ZERO, &world, Seed(0), &mut NoProbe);
    let lie = world.address_lie(ball, &stroke::footprint(club, &address), FORWARD);
    let input = StrokeInput::new(
        DeliveryInput {
            club,
            address,
            lie,
            attempt_seed: Seed(1),
        },
        FORWARD,
    );
    let shot = stroke::nominal_shot(&input, &world);
    let mut predictions = numbers(readout(shot.launch?.readout));
    predictions.extend(numbers([("carry_yd", units::to_yards(shot.carry_m()))]));
    Some(predictions)
}

#[cfg(test)]
mod tests {
    use golf_sim_validate::{Role, Tolerance};

    use super::*;

    #[test]
    fn measured_targets_pass() {
        let report = report(None).unwrap();
        assert!(!report.targets.is_empty());
        for dataset in &report.skipped_datasets {
            println!("{dataset}");
        }
        let failures = failures(&report).collect::<Vec<_>>();
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn spin_report_held_out_angles_are_within_scatter() {
        let report = report(None).unwrap();
        let (fit, held_out): (Vec<_>, Vec<_>) = report
            .targets
            .iter()
            .filter(|target| target.source == "SPIN07")
            .partition(|target| target.role == Role::Fit);
        assert_eq!(fit.len(), 6);
        assert_eq!(held_out.len(), 2);
        for name in ["out_speed_mps", "out_angle_deg", "out_spin_rad_s"] {
            let fit_mae = fit
                .iter()
                .map(|target| target.quantities[name].residual.unwrap().abs())
                .sum::<Real>()
                / fit.len() as Real;
            for row in &held_out {
                let value = &row.quantities[name];
                let Some(Tolerance::Sd(scatter)) = value.source_tolerance else {
                    panic!("{} {name} has no stated scatter", row.id);
                };
                let error = value.residual.unwrap().abs();
                assert!(
                    error <= fit_mae + scatter,
                    "{} {name}: {error} > {fit_mae} + {scatter}",
                    row.id
                );
                println!(
                    "{} {name}: |error| {error:.3}, fit MAE {fit_mae:.3}, scatter {scatter:.3}",
                    row.id
                );
            }
        }
    }
}
