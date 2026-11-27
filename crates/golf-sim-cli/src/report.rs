//! Balance reports from completed batch plays.

use std::{collections::BTreeMap, io};

use golf_sim::{
    base::{Real, Surface, Vec2, Vec3, fm, num::rad_to_deg, variant::Variant},
    conditions::lies::Sit,
    course::{
        course::{Course, Tee},
        legal::Area,
    },
    records::hole::{Entry, HoleRecord, Refusal},
};
use serde::Serialize;

pub fn lie_name(sit: Sit) -> &'static str {
    match sit {
        Sit::Up => "up",
        Sit::Normal => "normal",
        Sit::Down => "down",
    }
}

pub struct Play {
    hole: i64,
    day: String,
    profile: String,
    par: i64,
    seed: i64,
    strokes: usize,
    penalties: usize,
    holed: bool,
    fairway: bool,
    green_in_regulation: bool,
    putts: usize,
    finishes: Vec<Finish>,
}

#[derive(Serialize)]
struct Finish {
    seed: i64,
    stroke: usize,
    lie: &'static str,
    surface: String,
    target: &'static str,
    distance_m: Real,
    bearing_deg: Option<Real>,
}

impl Play {
    pub fn from_record(
        course: &Course,
        record: &HoleRecord,
        day: &str,
        profile: &str,
        strokes: usize,
        penalties: usize,
        holed: bool,
    ) -> Result<Self, Refusal> {
        record.check_course(course)?;
        let setup = &record.setup;
        let hole = course.hole(setup.hole).expect("checked hole");
        let tee = setup.tee(course).expect("checked tee");
        let flag = setup.cup(course).expect("checked cup").position;
        let landing = landing_area(tee);
        let legal = course.legal();
        let mut play = Self {
            hole: setup.hole,
            day: day.into(),
            profile: profile.into(),
            par: hole.par,
            seed: setup.seed.0,
            strokes,
            penalties,
            holed,
            fairway: false,
            green_in_regulation: false,
            putts: 0,
            finishes: Vec::new(),
        };
        let surface_at = |point| {
            course
                .material_at(point)
                .map_or(Surface::Rough, |material| material.kind)
        };
        let mut from_surface = surface_at(tee.position);
        let mut score = 0;
        for entry in &record.entries {
            match entry {
                Entry::Stroke(stroke) => {
                    score += 1;
                    let facts = &stroke.facts;
                    let surface = facts.lie.surface;
                    let on_green = surface == Surface::Green
                        && legal.area_at(facts.rest, &[], &hole.green.outline) == Area::Green;
                    let finished = holed && usize::try_from(score) == Ok(strokes);
                    let number = play.finishes.len() + 1;
                    play.fairway |= hole.par > 3 && number == 1 && surface == Surface::Fairway;
                    play.green_in_regulation |=
                        (on_green || finished) && score <= hole.par.saturating_sub(2);
                    play.putts += usize::from(from_surface == Surface::Green);
                    from_surface = surface;
                    let (target, point) = match landing {
                        Some(point) if !on_green && !finished => ("landing_area", point),
                        _ => ("flag", flag),
                    };
                    let offset = horizontal(point - facts.rest);
                    let distance_m = offset.length();
                    play.finishes.push(Finish {
                        seed: play.seed,
                        stroke: number,
                        lie: lie_name(facts.lie.sit),
                        surface: surface.name().to_ascii_lowercase(),
                        target,
                        distance_m,
                        bearing_deg: (distance_m > 0.0)
                            .then(|| rad_to_deg(fm::atan2(offset.x, -offset.y)).rem_euclid(360.0)),
                    });
                }
                Entry::Decision(decision) => {
                    if let Some(Variant::Int(strokes)) = decision.ruling.get("strokes") {
                        score = strokes.value();
                    }
                    if let Some(Variant::Vector3(spot)) = decision.ruling.get("spot") {
                        let [x, y, z] = spot.values;
                        let spot = Vec3::new(x, y, z);
                        if spot.is_finite() {
                            from_surface = surface_at(spot);
                        }
                    }
                }
            }
        }
        Ok(play)
    }
}

fn horizontal(point: Vec3) -> Vec2 {
    Vec2::new(point.x, point.z)
}

fn landing_area(tee: &Tee) -> Option<Vec3> {
    // Use the same 230 m look-ahead as the caddie on the course's play line.
    let mut distance = 0.0;
    let mut segments = tee.play_line.windows(2).peekable();
    while let Some(segment) = segments.next() {
        distance += horizontal(segment[1] - segment[0]).length();
        if distance >= 230.0 && segments.peek().is_some() {
            return Some(segment[1]);
        }
    }
    None
}

#[derive(Serialize)]
pub struct Report {
    holes: Vec<HoleReport>,
}

#[derive(Serialize)]
struct HoleReport {
    hole: i64,
    day: String,
    profile: String,
    par: i64,
    plays: usize,
    holed: usize,
    score_counts: BTreeMap<usize, usize>,
    mean: Real,
    std_dev: Real,
    fairways_hit: usize,
    greens_in_regulation: usize,
    putts: usize,
    penalties: usize,
    finishes: Vec<Finish>,
}

impl Report {
    pub fn from_plays(mut plays: Vec<Play>) -> Self {
        plays.sort_unstable_by(|a, b| {
            (a.hole, &a.day, &a.profile, a.seed).cmp(&(b.hole, &b.day, &b.profile, b.seed))
        });
        let mut plays = plays.into_iter().peekable();
        let mut holes = Vec::new();
        while let Some(play) = plays.next() {
            let mut report = HoleReport {
                hole: play.hole,
                day: play.day.clone(),
                profile: play.profile.clone(),
                par: play.par,
                plays: 0,
                holed: 0,
                score_counts: BTreeMap::new(),
                mean: 0.0,
                std_dev: 0.0,
                fairways_hit: 0,
                greens_in_regulation: 0,
                putts: 0,
                penalties: 0,
                finishes: Vec::new(),
            };
            report.add(play);
            while let Some(play) = plays.next_if(|play| {
                play.hole == report.hole && play.day == report.day && play.profile == report.profile
            }) {
                report.add(play);
            }
            let n = report.plays as Real;
            report.mean = report
                .score_counts
                .iter()
                .map(|(&score, &count)| score as Real * count as Real)
                .sum::<Real>()
                / n;
            let variance = report
                .score_counts
                .iter()
                .map(|(&score, &count)| {
                    let delta = score as Real - report.mean;
                    count as Real * delta * delta
                })
                .sum::<Real>()
                / n;
            report.std_dev = fm::sqrt(variance);
            holes.push(report);
        }
        Self { holes }
    }

    pub fn write_text(&self, mut out: impl io::Write) -> io::Result<()> {
        writeln!(
            out,
            "Hole Day      Profile Par Plays Holed   Mean StdDev Fairways GIR Putts Penalties Score:Count"
        )?;
        for row in &self.holes {
            write!(
                out,
                "{:>4} {:<8} {:<7} {:>3} {:>5} {:>5} {:>6.2} {:>6.2} {:>8} {:>3} {:>5} {:>9}",
                row.hole,
                row.day,
                row.profile,
                row.par,
                row.plays,
                row.holed,
                row.mean,
                row.std_dev,
                row.fairways_hit,
                row.greens_in_regulation,
                row.putts,
                row.penalties
            )?;
            for (score, count) in &row.score_counts {
                write!(out, " {score}:{count}")?;
            }
            writeln!(out)?;
        }
        writeln!(
            out,
            "\nHole Day      Profile                 Seed Stroke Lie    Surface    Target       Distance(m) Bearing(deg)"
        )?;
        for row in &self.holes {
            for finish in &row.finishes {
                let bearing = finish
                    .bearing_deg
                    .map_or_else(|| "-".into(), |angle| format!("{angle:.2}"));
                writeln!(
                    out,
                    "{:>4} {:<8} {:<7} {:>20} {:>6} {:<6} {:<10} {:<12} {:>11.2} {bearing:>12}",
                    row.hole,
                    row.day,
                    row.profile,
                    finish.seed,
                    finish.stroke,
                    finish.lie,
                    finish.surface,
                    finish.target,
                    finish.distance_m
                )?;
            }
        }
        Ok(())
    }
}

impl HoleReport {
    fn add(&mut self, play: Play) {
        self.plays += 1;
        self.holed += usize::from(play.holed);
        *self.score_counts.entry(play.strokes).or_default() += 1;
        self.fairways_hit += usize::from(play.fairway);
        self.greens_in_regulation += usize::from(play.green_in_regulation);
        self.putts += play.putts;
        self.penalties += play.penalties;
        self.finishes.extend(play.finishes);
    }
}
