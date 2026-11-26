use std::{
    io::{self, Write},
    path::PathBuf,
};

use clap::{Args, Subcommand};
use golf_sim::{
    base::{ClubId, Real, Seed, Vec3, fm, trace::NoProbe},
    conditions::{grounds::CourseGround, wear::WearMarkKind, world::World},
    course::course::Course,
    physics::{
        facade::standard_air,
        plane::ConstantAir,
        values::{EndReason, EventKind, ShotEvent, Zone},
    },
    stroke::{
        delivery::DeliveryInput,
        results::{End, StrokeOutcome},
        stroke::{self, StrokeInput},
    },
};
use serde_json::{Value, json};

use super::Outcome;

#[derive(Subcommand)]
pub enum Command {
    /// Play one stroke.
    Stroke(StrokeArgs),
}

#[derive(Args)]
pub struct StrokeArgs {
    #[arg(long, value_name = "DIR")]
    course: PathBuf,
    #[arg(long)]
    hole: i64,
    #[arg(long, value_name = "ID")]
    club: String,
    #[arg(long, default_value_t = 1, allow_negative_numbers = true)]
    seed: i64,
    /// Tee id. Defaults to the middle tee when available.
    #[arg(long, value_name = "ID")]
    tee: Option<String>,
    /// Compass bearing in degrees clockwise from north. Defaults to the tee's aim.
    #[arg(long, value_name = "DEGREES", allow_negative_numbers = true)]
    aim: Option<Real>,
}

pub fn run(command: Command) -> Outcome {
    match command {
        Command::Stroke(args) => play(args),
    }
}

fn play(args: StrokeArgs) -> Outcome {
    let club =
        ClubId::from_name(&args.club).ok_or_else(|| format!("unknown club {:?}", args.club))?;
    let course = Course::open(&args.course)
        .map_err(|error| format!("{}: {error}", args.course.display()))?;
    let hole = course
        .hole(args.hole)
        .ok_or_else(|| format!("{}: no hole {}", args.course.display(), args.hole))?;
    let tee = if let Some(id) = args.tee {
        hole.tees
            .iter()
            .find(|tee| tee.id == id)
            .ok_or_else(|| format!("hole {} has no tee {id:?}", args.hole))?
    } else {
        hole.tees
            .iter()
            .find(|tee| tee.id == "middle")
            .unwrap_or(&hole.tees[0])
    };
    let aim = if let Some(degrees) = args.aim {
        if !degrees.is_finite() {
            return Err("--aim must be a finite compass bearing".into());
        }
        let radians = degrees.rem_euclid(360.0).to_radians();
        Vec3::new(fm::sin(radians), 0.0, -fm::cos(radians))
    } else {
        Vec3::new(tee.aim.x, 0.0, tee.aim.z).normalized()
    };
    if !aim.is_finite() || aim == Vec3::ZERO {
        return Err(format!(
            "tee {:?} on hole {} has no horizontal aim",
            tee.id, args.hole
        )
        .into());
    }
    let cup = hole
        .pin_zones
        .iter()
        .filter(|zone| zone.position == "middle")
        .chain(hole.pin_zones.iter())
        .find_map(|zone| zone.cups.first().map(|cup| cup.position));
    let data = course.data();
    let mut air = standard_air();
    air.pressure_pa = World::station_pressure_pa(data.climate.altitude_m.unwrap_or(0.0));
    if let Some(july) = data
        .climate
        .monthly
        .iter()
        .find(|month| month.month == Some(7.0))
    {
        if let Some([low, high]) = july.temp_c {
            air.temperature_c = low + (high - low) * 0.7;
        }
        air.relative_humidity = july.humidity.unwrap_or(air.relative_humidity);
        let from = july.wind_from_deg.unwrap_or(0.0).to_radians();
        air.mean_wind_mps =
            Vec3::new(-fm::sin(from), 0.0, fm::cos(from)) * july.wind_mps.unwrap_or(0.0);
    }
    let green_speed = data
        .maintenance
        .everyday
        .as_ref()
        .and_then(|setup| setup.green_speed_m)
        .map_or(3.05, |[low, high]| (low + high) * 0.5);
    let ground = CourseGround::new(&course, 0.5, 0.3, green_speed);
    let world = World::on_course(ground, ConstantAir::new(air), cup, 9.0 * 3600.0);
    let seed = Seed(args.seed);
    let ball = stroke::place(tee.position, &world, seed.derive("tee"), &mut NoProbe);
    let address = stroke::stock(club);
    let lie = world.address_lie(ball, &stroke::footprint(club, &address), aim);
    let input = StrokeInput::new(
        DeliveryInput {
            club,
            address,
            lie,
            attempt_seed: seed.derive("attempt/0"),
        },
        aim,
    );
    let result = stroke::play(
        &input,
        stroke::nominal_execution(&input),
        &world,
        &mut NoProbe,
    );
    let output = json!({
        "outcome": outcome_json(&result.outcome),
        "rest": point(result.outcome.rest),
        "carry_m": result.outcome.carry_m,
        "end_reason": end_reason(result.playback.end_reason),
        "events": result.playback.events.iter().map(event_json).collect::<Vec<_>>(),
    });
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, &output)?;
    writeln!(stdout)?;
    Ok(())
}

fn point(point: Vec3) -> [Real; 3] {
    [point.x, point.y, point.z]
}

fn outcome_json(outcome: &StrokeOutcome) -> Value {
    let end = match outcome.end {
        End::AtRest => "at_rest",
        End::Holed => "holed",
        End::InWater => "in_water",
        End::TimedOut => "timed_out",
    };
    let marks: Vec<_> = outcome
        .marks
        .iter()
        .map(|mark| {
            json!({
                "kind": match mark.kind {
                    WearMarkKind::Divot => "divot",
                    WearMarkKind::PitchMark => "pitch_mark",
                },
                "start": point(mark.start),
                "end": point(mark.end),
                "depth_m": mark.depth_m,
                "surface": mark.surface.name().to_ascii_lowercase(),
            })
        })
        .collect();
    json!({
        "end": end,
        "start": point(outcome.start),
        "rest": point(outcome.rest),
        "rest_surface": outcome.rest_surface.name().to_ascii_lowercase(),
        "ball_radius_m": outcome.ball_radius_m,
        "carry_m": outcome.carry_m,
        "total_m": outcome.total_m,
        "embedded": outcome.embedded,
        "penalty_entry": outcome.penalty_entry.map(point),
        "marks": marks,
    })
}

fn end_reason(reason: EndReason) -> &'static str {
    match reason {
        EndReason::Rest => "rest",
        EndReason::Holed => "holed",
        EndReason::Water => "water",
        EndReason::Timeout => "timeout",
        EndReason::InvalidRelease => "invalid_release",
        EndReason::InvalidInput => "invalid_input",
    }
}

fn event_json(event: &ShotEvent) -> Value {
    let kind = match event.kind {
        EventKind::Impact => "impact",
        EventKind::Apex => "apex",
        EventKind::Landing => "landing",
        EventKind::Bounce => "bounce",
        EventKind::RollStart => "roll_start",
        EventKind::PureRoll => "pure_roll",
        EventKind::Obstacle => "obstacle",
        EventKind::Canopy => "canopy",
        EventKind::SurfaceEntered => "surface_entered",
        EventKind::ZoneEntered => "zone_entered",
        EventKind::LipOut => "lip_out",
        EventKind::Holed => "holed",
        EventKind::Rest => "rest",
        EventKind::Release => "release",
        EventKind::CupEntry => "cup_entry",
    };
    let zone = match event.zone {
        Zone::InPlay => "in_play",
        Zone::PenaltyArea => "penalty_area",
        Zone::OutOfBounds => "out_of_bounds",
    };
    json!({
        "kind": kind,
        "time_s": event.time_s,
        "position": point(event.position),
        "velocity": point(event.velocity),
        "spin": point(event.spin),
        "surface": event.surface.name().to_ascii_lowercase(),
        "zone": zone,
    })
}
