use std::{collections::BTreeMap, path::PathBuf};

use clap::{Args, Subcommand};
use golf_sim::{
    base::{ClubId, Real, Surface, Vec3, fm, trace::NoProbe},
    physics::{
        air, facade as sim, roll,
        samplers::GroundSample,
        tunables as t,
        values::{ClubCategory, EndReason, EventKind, ShotEvent, units},
    },
};
use golf_sim_validate::{Conditions, Corpus, Fit, Kind, Role, Target, TargetFile};

use super::Outcome;
use crate::validate_model::{self as model, DeliveryProbe, Inputs};

#[derive(Subcommand)]
pub enum Command {
    /// Produce calibration values from the physics model.
    Fit(FitArgs),
}

#[derive(Args)]
pub struct FitArgs {
    /// Read the public validation corpus from DIR.
    #[arg(long, global = true, value_name = "DIR")]
    data: Option<PathBuf>,
    #[command(subcommand)]
    job: Job,
}

#[derive(Subcommand)]
enum Job {
    /// Fit club impact parameters to measured deliveries and face-map measurements.
    Impact,
    /// Fit fairway resistance to the published driver run-out.
    Runout,
    /// Measure the calm launches used to check the Python flight solver.
    Parity,
    #[command(hide = true)]
    Launches,
}

pub fn run(Command::Fit(args): Command) -> Outcome {
    let corpus = Corpus::load_default(args.data.as_deref())?;
    match args.job {
        Job::Impact => fit_impact(&corpus),
        Job::Runout => fit_runout(&corpus),
        Job::Parity => parity(&corpus),
        Job::Launches => launch_inputs(&corpus),
    }
}

type Row<'a> = (&'a Target, &'a Conditions);

fn rows(file: &TargetFile, kind: Kind) -> impl Iterator<Item = Row<'_>> {
    file.targets
        .iter()
        .filter(move |target| target.kind == kind)
        .map(|target| (target, &file.conditions))
}

fn fit_rows(corpus: &Corpus, role: Fit, kind: Kind) -> impl Iterator<Item = Row<'_>> {
    corpus
        .files()
        .iter()
        .filter(move |file| file.fits.contains(&role))
        .flat_map(move |file| rows(file, kind))
}

fn skip(job: &str, role: Fit) -> Outcome {
    println!("Skipped {job}. No targets with the {role} role.");
    Ok(())
}

fn measurement(target: &Target, name: &str) -> Option<Real> {
    if target.role == Role::Fit && !target.held_out.iter().any(|held| held == name) {
        target.measured.get(name)?.as_number()
    } else {
        None
    }
}

fn fit_impact(corpus: &Corpus) -> Outcome {
    let deliveries: Vec<_> = fit_rows(corpus, Fit::Impact, Kind::Delivery)
        .filter(|(target, _)| target.role == Role::Fit)
        .collect();
    if deliveries.is_empty() {
        return skip("IMPACT", Fit::Impact);
    }
    let face: Vec<_> = corpus
        .files()
        .iter()
        .filter(|file| file.source == "R47")
        .flat_map(|file| rows(file, Kind::Delivery))
        .filter(|(target, _)| measurement(target, "spin_rpm_change").is_some())
        .collect();
    let mut fits = BTreeMap::new();
    for &(target, conditions) in &deliveries {
        let input = Inputs(target);
        let Some(loft) = input.number("dynamic_loft_deg") else {
            continue;
        };
        let club = input.club().ok_or("unknown calibration club")?;
        let probe = delivery_probe(target, conditions)?;
        let references =
            ["ball_mph", "launch_deg", "spin_rpm"].map(|name| measurement(target, name));
        let values = if club == ClubId::Driver {
            search(
                [0.84, -0.35, 3500.0, 0.025],
                [0.04, 0.08, 400.0, 0.015],
                |p| {
                    impact_error(club, p, &probe, loft, references, &face, &deliveries)
                        .unwrap_or(Real::INFINITY)
                },
            )
            .to_vec()
        } else {
            search([0.77, 0.2], [0.04, 0.08], |[cor, tangential]| {
                impact_error(
                    club,
                    [cor, tangential, 0.0, 0.0],
                    &probe,
                    loft,
                    references,
                    &[],
                    &[],
                )
                .unwrap_or(Real::INFINITY)
            })
            .to_vec()
        };
        let values: Vec<_> = values
            .into_iter()
            .enumerate()
            .map(|(index, value)| snap(value, if index == 2 { 0.1 } else { 0.00001 }))
            .collect();
        let normal_speed = units::mph(input.number("club_mph").ok_or("missing club speed")?)
            * fm::cos(
                (loft - input.number("attack_deg").ok_or("missing attack angle")?).to_radians(),
            );
        if fits.insert(club, (values, normal_speed)).is_some() {
            return Err(format!("multiple impact anchors for {club}").into());
        }
    }
    if fits.is_empty() {
        return Err("no fit-role delivery anchors".into());
    }

    let [slope] = search([0.0], [0.004], |[slope]| {
        iron_error(&deliveries, &fits, slope).unwrap_or(Real::INFINITY)
    });
    let slope = snap(slope, 0.00001);
    for (club, (mut values, speed)) in fits {
        if sim::standard_club(club).category == ClubCategory::Iron {
            values[0] = snap(rebase_cor(values[0], speed, slope), 0.00001);
        }
        println!("FIT {}", serde_json::to_string(&(club.name(), values))?);
    }
    println!("IRON COR {slope}");
    Ok(())
}

fn delivery_probe<'a>(
    target: &'a Target,
    conditions: &Conditions,
) -> Result<DeliveryProbe<'a>, Box<dyn std::error::Error>> {
    DeliveryProbe::from_target(target, conditions)
        .ok_or_else(|| format!("invalid delivery inputs for {}", target.id).into())
}

fn impact_error(
    club: ClubId,
    [cor, tangential, moi, speed_slope]: [Real; 4],
    probe: &DeliveryProbe<'_>,
    loft: Real,
    measured: [Option<Real>; 3],
    face: &[Row<'_>],
    deliveries: &[Row<'_>],
) -> Option<Real> {
    if !(0.5..=0.92).contains(&cor) || !(-0.7..=0.6).contains(&tangential) {
        return None;
    }
    let mut spec = sim::standard_club(club);
    spec.cor = cor;
    spec.cor_speed_slope_delta = 0.0;
    spec.tangential_restitution_up = tangential;
    if club == ClubId::Driver {
        if !(2000.0..=5000.0).contains(&moi) || !(0.0..=0.09).contains(&speed_slope) {
            return None;
        }
        spec.moi_heel_toe = moi * 1e-7;
        spec.tangential_restitution_speed_slope = speed_slope;
    } else {
        spec.tangential_restitution_toe = tangential;
    }
    let strike = probe.strike(&spec, loft, 0.0, 0.0)?.readout;
    let predictions = [
        units::to_mph(strike.ball_speed_mps),
        strike.launch_angle_rad.to_degrees(),
        units::to_rpm(strike.spin_rate_rad_s),
    ];
    let scales = [1.5, 0.5, measured[2].unwrap_or(1.0) * 0.045];
    let mut error = predictions
        .into_iter()
        .zip(measured)
        .zip(scales)
        .filter_map(|((predicted, measured), scale)| Some(square((predicted - measured?) / scale)))
        .sum::<Real>();

    for &(target, conditions) in face {
        let input = Inputs(target);
        let probe = DeliveryProbe::from_target(target, conditions)?;
        let loft = input.number("dynamic_loft_deg")?;
        let centre = probe.strike(&spec, loft, 0.0, 0.0)?.readout.spin_rate_rad_s;
        let spot = probe
            .strike(
                &spec,
                loft,
                input.number("toe_mm")? * 0.001,
                input.number("up_mm")? * 0.001,
            )?
            .readout
            .spin_rate_rad_s;
        let measured = measurement(target, "spin_rpm_change")?;
        error += square((units::to_rpm(spot) - units::to_rpm(centre) - measured) / 240.0);
    }
    if club == ClubId::Driver {
        for &(target, conditions) in deliveries {
            let input = Inputs(target);
            if input.club() != Some(club) || input.number("dynamic_loft_deg").is_some() {
                continue;
            }
            if let Some(spin) = measurement(target, "spin_rpm") {
                let probe = DeliveryProbe::from_target(target, conditions)?;
                let (_, launch) = probe.loft_for_launch(&spec, 0.0, 0.0, [8.0, 30.0], 16)?;
                error += square((units::to_rpm(launch.readout.spin_rate_rad_s) - spin) / 180.0);
            }
        }
    }
    Some(error)
}

fn iron_error(
    deliveries: &[Row<'_>],
    fits: &BTreeMap<ClubId, (Vec<Real>, Real)>,
    slope: Real,
) -> Option<Real> {
    let mut error = 0.0;
    for (&club, (values, speed)) in fits {
        let mut spec = sim::standard_club(club);
        if spec.category != ClubCategory::Iron {
            continue;
        }
        spec.cor = rebase_cor(values[0], *speed, slope);
        spec.cor_speed_slope_delta = slope;
        spec.tangential_restitution_up = values[1];
        spec.tangential_restitution_toe = values[1];
        for &(target, conditions) in deliveries {
            let input = Inputs(target);
            if input.club() != Some(club) || input.number("dynamic_loft_deg").is_some() {
                continue;
            }
            if let Some(ball_mph) = measurement(target, "ball_mph") {
                let probe = DeliveryProbe::from_target(target, conditions)?;
                let (_, launch) = probe.loft_for_launch(&spec, 0.0, 0.0, [10.0, 45.0], 16)?;
                error += square(units::to_mph(launch.readout.ball_speed_mps) - ball_mph);
            }
        }
    }
    Some(error)
}

fn square(value: Real) -> Real {
    value * value
}

fn rebase_cor(cor: Real, normal_speed: Real, slope: Real) -> Real {
    cor - slope * (t::COR_REFERENCE_SPEED - normal_speed)
}

fn search<const N: usize>(
    mut point: [Real; N],
    mut steps: [Real; N],
    error: impl Fn([Real; N]) -> Real,
) -> [Real; N] {
    let mut best = error(point);
    for _ in 0..15 {
        for (axis, step) in steps.iter().enumerate() {
            let initial = point[axis];
            let mut chosen = initial;
            for direction in [-1.0, 1.0] {
                point[axis] = initial + direction * step;
                let value = error(point);
                if value < best {
                    best = value;
                    chosen = point[axis];
                }
            }
            point[axis] = chosen;
        }
        for step in &mut steps {
            *step *= 0.68;
        }
    }
    point
}

fn snap(value: Real, step: Real) -> Real {
    fm::floor(value / step + 0.5) * step
}

fn fit_runout(corpus: &Corpus) -> Outcome {
    let mut candidates = fit_rows(corpus, Fit::Runout, Kind::RunOut).peekable();
    if candidates.peek().is_none() {
        return skip("RUNOUT", Fit::Runout);
    }
    let rows: Vec<_> = candidates
        .filter_map(|(target, conditions)| {
            Some((target, conditions, measurement(target, "run_yd")?))
        })
        .collect();
    let (target, conditions, _) = rows.first().ok_or("no fit-role fairway run-out targets")?;
    if rows.iter().any(|(row, _, _)| row.inputs != target.inputs) {
        return Err("fairway run-out targets must share a launch".into());
    }
    let mean_yd = rows.iter().map(|(_, _, measured)| measured).sum::<Real>() / rows.len() as Real;
    let input = Inputs(target);
    let ground = model::ground(conditions).ok_or("unknown run-out surface")?;
    let air = model::air(input, conditions);
    let shot = model::launch_shot(input, &air, &ground).ok_or("invalid run-out launch")?;
    let landing = shot
        .first_event(EventKind::Landing)
        .ok_or("run-out shot did not land")?;
    let rolling = shot
        .first_event(EventKind::PureRoll)
        .ok_or("run-out shot did not roll")?;
    if shot.end_reason != EndReason::Rest || t::surface(rolling.surface).grass_drag != 0.0 {
        return Err(
            "run-out fit needs a shot that comes to rest on turf without grass drag".into(),
        );
    }
    let aim = shot.launch.ok_or("missing run-out launch")?.aim;
    let remaining = units::yards(mean_yd) - (rolling.position - landing.position).dot(aim);
    if remaining <= 0.0 {
        return Err("bounce and skid exceed the run-out target".into());
    }
    let roll_air = roll::Air {
        density: air::density(&air.air),
        viscosity: air::viscosity(&air.air),
        half_area_per_mass: 0.5 * t::BALL.area() / t::BALL.mass_kg,
    };
    let (mut lo, mut hi) = (0.01, 10.0);
    if coast(rolling, hi, &roll_air)? < remaining || coast(rolling, lo, &roll_air)? > remaining {
        return Err("run-out target falls outside the stimp search interval".into());
    }
    for _ in 0..30 {
        let mid = (lo + hi) * 0.5;
        if coast(rolling, mid, &roll_air)? < remaining {
            lo = mid
        } else {
            hi = mid
        }
    }
    println!("RUNOUT {}", (lo + hi) * 0.5);
    Ok(())
}

fn coast(start: &ShotEvent, stimp_m: Real, air: &roll::Air) -> Result<Real, &'static str> {
    let ground = GroundSample {
        surface: Surface::Green,
        green_speed_m: stimp_m,
        ..GroundSample::default()
    };
    let mut state = roll::State {
        velocity: start.velocity,
        spin: start.spin,
        motion: roll::Motion::Roll,
    };
    let mut travel = Vec3::ZERO;
    let mut time = 0.0;
    while time < t::MAX_SHOT_TIME_S {
        roll::step(
            &mut state,
            &ground,
            &t::BALL,
            air,
            t::GROUND_DT,
            &mut NoProbe,
        );
        travel += state.velocity * t::GROUND_DT;
        if state.motion == roll::Motion::Rest {
            return Ok(travel.length());
        }
        time += t::GROUND_DT;
    }
    Err("run-out coast did not reach rest")
}

fn parity(corpus: &Corpus) -> Outcome {
    let mut launches: Vec<_> = fit_rows(corpus, Fit::Parity, Kind::Launch).collect();
    if launches.is_empty() {
        return skip("flight parity", Fit::Parity);
    }
    launches.sort_by(|(left, _), (right, _)| left.id.cmp(&right.id));
    for (target, conditions) in launches {
        let inputs = Inputs(target);
        let air = model::air(inputs, conditions);
        let ground = model::ground(conditions).ok_or("unknown parity surface")?;
        let shot = model::launch_shot(inputs, &air, &ground).ok_or("invalid parity launch")?;
        let landing = shot
            .first_event(EventKind::Landing)
            .ok_or("parity launch did not land")?;
        let values = [
            units::to_yards(shot.carry_m()),
            units::to_yards(shot.apex_height_m()),
            shot.land_angle_rad().to_degrees(),
            landing.spin.length(),
        ];
        println!("FLIGHT {}", serde_json::to_string(&values)?);
    }
    Ok(())
}

fn launch_inputs(corpus: &Corpus) -> Outcome {
    let launch_roles = [Fit::Aero, Fit::CarryGain, Fit::Parity];
    for file in corpus.files() {
        let roles: Vec<_> = file
            .fits
            .iter()
            .filter(|role| launch_roles.contains(role))
            .collect();
        if roles.is_empty() {
            continue;
        }
        let launches: Vec<_> = rows(file, Kind::Launch)
            .map(|(target, _)| {
                serde_json::json!({
                    "id": target.id,
                    "inputs": target.inputs,
                    "measured": target.measured,
                    "use": target.role,
                    "held_out": target.held_out,
                })
            })
            .collect();
        if !launches.is_empty() {
            let group = serde_json::json!({"fits": roles, "targets": launches});
            println!("LAUNCHES {group}");
        }
    }
    Ok(())
}
