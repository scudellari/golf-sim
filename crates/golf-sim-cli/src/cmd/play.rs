use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
};

use clap::{Args, Subcommand, ValueEnum};
use golf_sim::{
    base::Seed,
    course::course::Course,
    hole::Hole,
    records::hole::HoleRecord,
    setup::{HoleSetup, Mode},
};

use super::Outcome;
use crate::protocol::{self, Acted, Decision, Operation, ProfileName, Refused, Reply, Success};

#[derive(Subcommand)]
pub enum Command {
    /// Play one hole: one JSON request per line on stdin, one reply per line on stdout.
    Play(PlayArgs),
}

#[derive(Args)]
pub struct PlayArgs {
    #[arg(long, value_name = "DIR")]
    course: PathBuf,
    #[arg(long)]
    hole: i64,
    #[arg(long, value_enum, default_value = "constant")]
    day: Day,
    #[arg(long, default_value_t = 1, allow_negative_numbers = true)]
    seed: i64,
    #[arg(long, value_enum, default_value = "caddie")]
    mode: InfoMode,
    /// Default skill profile for strokes that omit their swing.
    #[arg(long, value_enum)]
    profile: Option<ProfileName>,
}

#[derive(Clone, ValueEnum)]
enum Day {
    Constant,
}

#[derive(Clone, ValueEnum)]
enum InfoMode {
    Caddie,
    Tour,
}

pub fn run(command: Command) -> Outcome {
    let Command::Play(args) = command;
    let course = Course::open(&args.course)?;
    let mut setup = match args.day {
        Day::Constant => HoleSetup::standard(&course, args.hole, Seed(args.seed)),
    };
    setup.settings.info_mode = match args.mode {
        InfoMode::Caddie => Mode::Caddie,
        InfoMode::Tour => Mode::Tour,
    };
    let record = HoleRecord::begin(setup, HoleRecord::agent_driver("golf play"));
    let mut hole = Hole::resume(course.clone(), &record.to_bytes()?)?;
    if let Err(error) = serve(
        &mut hole,
        &course,
        args.profile,
        io::stdin().lock(),
        io::stdout().lock(),
    ) {
        eprintln!("golf play: {error}");
    }
    Ok(())
}

fn serve(
    hole: &mut Hole,
    course: &Course,
    profile: Option<ProfileName>,
    mut input: impl BufRead,
    mut output: impl Write,
) -> io::Result<()> {
    let mut line = Vec::new();
    while input.read_until(b'\n', &mut line)? != 0 {
        let reply = match protocol::read_request(&line) {
            Ok(request) => match operate(hole, course, profile, request.operation) {
                Ok(ok) => Reply::Ok { id: request.id, ok },
                Err(refused) => Reply::Refused {
                    id: request.id,
                    refused,
                },
            },
            Err((id, refused)) => Reply::Refused { id, refused },
        };
        protocol::write_line(&mut output, &reply)?;
        output.flush()?;
        line.clear();
    }
    Ok(())
}

fn operate(
    hole: &mut Hole,
    course: &Course,
    profile: Option<ProfileName>,
    operation: Operation,
) -> Result<Success, Refused> {
    let prompt = hole.prompt().kind;
    Ok(match operation {
        Operation::Observe => {
            let mode = hole.record().setup.settings.info_mode;
            Success::Observe(Box::new(hole.observe(mode).into()))
        }
        Operation::Legal => {
            let ball = hole.ball_lie().map(|ball| ball.spot);
            let aim = hole.advice().map(|advice| advice.first_aim);
            Success::Legal {
                actions: hole
                    .legal()
                    .into_iter()
                    .map(|action| Decision::offered(action, ball.zip(aim)))
                    .collect(),
            }
        }
        Operation::Preview { stroke } => {
            let preview = hole
                .preview(&stroke.choice(course))
                .map_err(|error| Refused::action(error, prompt))?;
            Success::Preview(preview.into())
        }
        Operation::Act { action } => {
            let step = hole
                .act(action.action(course, profile)?)
                .map_err(|error| Refused::action(error, prompt))?;
            Success::Act(Acted::new(hole, step.stroke.map(|stroke| stroke.outcome)))
        }
        Operation::Record => Success::Record {
            bytes: protocol::record(hole)?,
        },
        Operation::Save { path } => return protocol::save(hole, path),
    })
}
