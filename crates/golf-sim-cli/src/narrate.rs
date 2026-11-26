//! Readable accounts of stroke traces.

use std::io::{self, Write};

use golf_sim::base::{
    Real, Surface,
    trace::{Phase, PhaseTrace, Source, Trace, TraceValue},
};

pub fn write(out: &mut impl Write, trace: &Trace) -> io::Result<()> {
    let mut previous = None;
    for entry in trace.phases() {
        if previous != Some(entry.phase) || !matches!(entry.phase, Phase::Launch | Phase::Skid) {
            phase(out, entry, previous)?;
        }
        previous = Some(entry.phase);
    }
    Ok(())
}

fn phase(out: &mut impl Write, entry: &PhaseTrace, previous: Option<Phase>) -> io::Result<()> {
    match entry.phase {
        Phase::Lie => summary(
            out,
            entry,
            &format!("The ball started on the {}", surface(entry)),
            &[
                ("ball_height_m", "ball height above the soil"),
                ("ground_height_m", "ground elevation"),
                ("sit_draw", "lie draw"),
            ],
        ),
        Phase::Delivery if find(entry, "club_speed").is_none() => summary(
            out,
            entry,
            "The swing model set up the shot",
            &[
                ("club_speed_mps", "stock club speed"),
                ("fault", "timing fault"),
                ("hidden.face_deg", "face variation draw"),
            ],
        ),
        Phase::Delivery => summary(
            out,
            entry,
            "The club arrived at the ball",
            &[
                ("club_speed", "club speed"),
                ("attack", "angle of attack"),
                ("dynamic_loft", "delivered loft"),
            ],
        ),
        Phase::Impact => summary(
            out,
            entry,
            "The club struck the ball",
            &[
                ("club_speed_at_ball", "speed at contact"),
                ("strike_toe", "toe offset"),
                ("strike_up", "vertical strike offset"),
                ("turf_contact_depth", "turf contact depth"),
            ],
        ),
        Phase::Launch => summary(
            out,
            entry,
            "The ball left the clubface",
            &[
                ("ball_speed", "ball speed"),
                ("launch_angle", "launch angle"),
                ("spin_rate", "spin rate"),
            ],
        ),
        Phase::Flight if previous == Some(Phase::Bounce) => summary(
            out,
            entry,
            "The ball lifted again after the bounce",
            &[("time_s", "time since impact"), ("speed", "speed")],
        ),
        Phase::Flight => summary(
            out,
            entry,
            "The ball was airborne",
            &[
                ("apex_height_m", "apex elevation"),
                ("apex_time_s", "time at apex"),
                ("air_density", "air density"),
                ("GRAVITY", "gravity"),
            ],
        ),
        Phase::Bounce => summary(
            out,
            entry,
            &format!("The ball bounced on the {}", surface(entry)),
            &[
                ("incidence_speed", "incoming speed"),
                ("incidence_angle", "angle to the ground"),
                ("firmness", "firmness"),
                ("normal_restitution", "normal restitution"),
                ("spin_in", "spin before contact"),
                ("spin_out", "spin after contact"),
            ],
        ),
        Phase::Skid => summary(
            out,
            entry,
            "The ball skidded",
            &[
                ("slip_speed", "initial slip speed"),
                ("skid_friction", "skid friction"),
            ],
        ),
        Phase::Roll => roll(out, entry),
        Phase::Cup => cup(out, entry),
        Phase::Ruling => summary(out, entry, "The stroke received a ruling", &[]),
    }
}

fn roll(out: &mut impl Write, entry: &PhaseTrace) -> io::Result<()> {
    if find(entry, "rolling_started").is_some() {
        return summary(
            out,
            entry,
            "The ball began to roll without slipping",
            &[("rolling_started", "speed at transition")],
        );
    }
    let (sentence, speed) = if find(entry, "stopped_below_speed_threshold").is_some() {
        (
            "The ball rolled to a stop below the speed threshold",
            "stopped_below_speed_threshold",
        )
    } else {
        (
            "Rolling resistance brought the ball to a stop",
            "stopped_by_resistance",
        )
    };
    summary(
        out,
        entry,
        sentence,
        &[
            (speed, "speed before stopping"),
            ("stop_speed", "stop threshold"),
            ("resistance", "rolling resistance"),
            ("green_speed", "green speed"),
        ],
    )
}

fn cup(out: &mut impl Write, entry: &PhaseTrace) -> io::Result<()> {
    for (key, sentence) in [
        ("drop_started_s", "The ball began dropping into the cup"),
        ("holed_at_s", "The ball was holed"),
        ("airborne_entry_s", "The airborne ball entered the cup"),
        (
            "airborne_lip_out_s",
            "The airborne ball struck the lip and stayed out",
        ),
        ("flagstick_contact_s", "The ball struck the flagstick"),
    ] {
        if find(entry, key).is_some() {
            return summary(out, entry, sentence, &[(key, "time since impact")]);
        }
    }
    let sentence = if find(entry, "capture").is_some() {
        "The cup caught the rolling ball"
    } else if find(entry, "lip_out").is_some() {
        "The rolling ball lipped out"
    } else if find(entry, "crossing_to_flagstick").is_some() {
        "The ball crossed the rim toward the flagstick"
    } else {
        "The ball reached the cup"
    };
    summary(
        out,
        entry,
        sentence,
        &[
            ("rim_speed", "speed at the rim"),
            ("capture_speed", "capture speed at this offset"),
            ("cup_radius", "cup radius"),
        ],
    )
}

fn summary(
    out: &mut impl Write,
    entry: &PhaseTrace,
    sentence: &str,
    fields: &[(&str, &str)],
) -> io::Result<()> {
    write!(out, "{sentence}")?;
    let mut values = fields
        .iter()
        .filter_map(|(name, label)| find(entry, name).map(|value| (label, value)))
        .peekable();
    if values.peek().is_some() {
        write!(out, " (")?;
        while let Some((label, value)) = values.next() {
            write!(out, "{label} ")?;
            measurement(out, value)?;
            if values.peek().is_some() {
                write!(out, ", ")?;
            }
        }
        write!(out, ")")?;
    }
    writeln!(out, ".")
}

fn measurement(out: &mut impl Write, value: &TraceValue) -> io::Result<()> {
    let (number, unit) = match value.unit.as_str() {
        "rad" => (value.value.to_degrees(), "deg"),
        "rad/s" => (value.value * (60.0 / std::f64::consts::TAU), "rpm"),
        "1" => (value.value, ""),
        unit => (value.value, unit),
    };
    let magnitude = number.abs();
    if number != 0.0 && magnitude < 0.001 {
        write!(out, "{number:.3e}")?;
    } else if magnitude < 0.1 {
        write!(out, "{number:.4}")?;
    } else {
        write!(out, "{number:.3}")?;
    }
    if !unit.is_empty() {
        write!(out, " {unit}")?;
    }
    match &value.source {
        Source::Computed => write!(out, " [computed {}]", value.name),
        Source::Tunable { name } => write!(out, " [tunable {name}]"),
        Source::Turf { name } => write!(out, " [turf {name}]"),
        Source::Course { name } => write!(out, " [course {name}]"),
        Source::SeedDraw { name } => write!(out, " [seed draw {name}]"),
    }
}

fn find<'a>(entry: &'a PhaseTrace, name: &str) -> Option<&'a TraceValue> {
    entry.values.iter().find(|value| value.name == name)
}

fn surface(entry: &PhaseTrace) -> String {
    find(entry, "surface")
        .and_then(|value| {
            Surface::ALL
                .iter()
                .find(|surface| Real::from(**surface as u8) == value.value)
        })
        .map_or_else(
            || "ground".into(),
            |surface| surface.name().to_ascii_lowercase().replace('_', " "),
        )
}
