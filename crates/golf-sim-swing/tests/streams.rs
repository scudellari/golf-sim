use std::{
    convert::Infallible,
    path::{Path, PathBuf},
};

use golf_sim_base::{
    Real, Seed, Vec2, corpus,
    digest::object,
    variant::{Array, Components, Dict, FloatWidth, Variant},
};
use golf_sim_swing::{
    execution::Execution,
    session::{Phase, SwingSession, SwingView},
    spec::{StrikeMode, SwingSpec},
};
use golf_sim_testkit::tolerance;

const CASES: usize = 806;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/swing/streams/fixture.bin")
}

macro_rules! record {
    ($($key:ident: $value:expr),+ $(,)?) => {
        object([$( (stringify!($key), ($value).into()) ),+])
    };
}

fn field<'a>(dict: &'a Dict, key: &str) -> &'a Variant {
    dict.get(key).unwrap_or_else(|| panic!("missing {key}"))
}

fn dict(value: &Variant) -> &Dict {
    value.as_dict().unwrap()
}

fn items(value: &Variant) -> &[Variant] {
    let Variant::Array(array) = value else {
        panic!("expected an array");
    };
    &array.items
}

fn integer(input: &Dict, key: &str) -> i64 {
    let Variant::Int(value) = field(input, key) else {
        panic!("expected an integer at {key}");
    };
    value.value()
}

fn number(input: &Dict, key: &str) -> Real {
    match field(input, key) {
        Variant::Float(value) => value.value(),
        Variant::Int(value) => value.value() as Real,
        _ => panic!("expected a number at {key}"),
    }
}

fn vec2(input: &Dict, key: &str) -> Vec2 {
    let Variant::Vector2(vector) = field(input, key) else {
        panic!("expected a Vector2 at {key}");
    };
    let [x, y] = vector.values;
    Vec2::new(x, y)
}

fn vector(point: Vec2) -> Variant {
    Variant::Vector2(Components {
        values: [point.x, point.y],
        width: FloatWidth::Double,
    })
}

fn spec(input: &Dict) -> SwingSpec {
    SwingSpec {
        target_power: number(input, "target_power"),
        power_cap: number(input, "power_cap"),
        meter_rate_per_s: number(input, "meter_rate_per_s"),
        tempo_ratio: number(input, "tempo_ratio"),
        ring_min_s: number(input, "ring_min_s"),
        ring_max_s: number(input, "ring_max_s"),
        window_early_s: number(input, "window_early_s"),
        window_late_s: number(input, "window_late_s"),
        timeout_after_s: number(input, "timeout_after_s"),
        strike_mode: match integer(input, "strike_mode") {
            0 => StrikeMode::Reticle,
            1 => StrikeMode::None,
            value => panic!("unknown strike mode {value}"),
        },
        drift_bias_m: vec2(input, "drift_bias_m"),
        drift_start_sd_m: vec2(input, "drift_start_sd_m"),
        drift_slide_sd_mps: vec2(input, "drift_slide_sd_mps"),
        steer_vertical: match field(input, "steer_vertical") {
            Variant::Bool(value) => *value,
            _ => panic!("expected a boolean at steer_vertical"),
        },
        downswing_slide_gain: number(input, "downswing_slide_gain"),
        wobble_m: vec2(input, "wobble_m"),
    }
}

struct Stream<'a> {
    spec: SwingSpec,
    start_usec: i64,
    seed: Seed,
    events: &'a [Variant],
}

impl<'a> Stream<'a> {
    fn read(input: &'a Dict) -> Self {
        Self {
            spec: spec(dict(field(input, "spec"))),
            start_usec: integer(input, "start_usec"),
            seed: Seed(integer(input, "seed")),
            events: items(field(input, "events")),
        }
    }

    fn session(&self) -> SwingSession {
        SwingSession::new(self.spec, self.start_usec, self.seed)
    }
}

fn apply(session: &mut SwingSession, event: &Dict) {
    let time = integer(event, "usec");
    match field(event, "kind").as_str().unwrap() {
        "press" => session.press(time),
        "steer" => session.steer(time, vec2(event, "delta_m")),
        "advance" => session.advance(time),
        kind => panic!("unknown event {kind}"),
    }
}

fn execution_snapshot(execution: Execution) -> Variant {
    record! {
        power: execution.power,
        timing_error_s: execution.timing_error_s,
        timing_fault: execution.timing_fault,
        timed_out: Variant::Bool(execution.timed_out),
        has_strike: Variant::Bool(execution.strike_m.is_some()),
        strike_m: vector(execution.strike_m.unwrap_or(Vec2::ZERO)),
        impact_usec: execution.impact_usec,
        backswing_s: execution.backswing_s,
        ring_s: execution.ring_s,
    }
}

fn phase_code(phase: Phase) -> i64 {
    match phase {
        Phase::Backswing => 0,
        Phase::Downswing => 1,
        Phase::Done => 2,
    }
}

fn view_snapshot(view: SwingView) -> Variant {
    record! {
        phase: phase_code(view.phase),
        target_power: view.target_power,
        power_cap: view.power_cap,
        meter: view.meter,
        ring_s: view.ring_s,
        ring_min_s: view.ring_min_s,
        ring_max_s: view.ring_max_s,
        window_early_s: view.window_early_s,
        window_late_s: view.window_late_s,
        timeout_after_s: view.timeout_after_s,
        since_power_s: view.since_power_s,
        since_impact_s: view.since_impact_s,
        club_speed_mps: view.club_speed_mps,
        has_reticle: Variant::Bool(view.has_reticle),
        reticle_m: vector(view.reticle_m),
        timing_error_s: view.timing_error_s,
        timed_out: Variant::Bool(view.timed_out),
    }
}

fn replay(id: &str, stream: Stream<'_>, expected: &Dict) -> Variant {
    let mut session = stream.session();
    let states = items(field(expected, "states"));
    let queries = items(field(expected, "views"));
    let mut actual_states = Vec::new();
    let mut actual_views = Vec::new();
    let mut observe = |session: &SwingSession, query: &Variant| {
        // The recorder stores the query stamp in the view envelope.
        let query = dict(query);
        let stamp = integer(query, "usec");
        let view = session.view_at(stamp);
        assert_eq!(
            phase_code(view.phase),
            integer(dict(field(query, "view")), "phase"),
            "{id} phase at {stamp}"
        );
        actual_views.push(record! {
            usec: stamp,
            view: view_snapshot(view),
            reticle: vector(session.reticle_at(stamp)),
        });
    };
    let mut queries = queries.iter().peekable();
    for (index, event) in stream.events.iter().enumerate() {
        let event = dict(event);
        let stamp = integer(event, "usec");
        while queries
            .peek()
            .is_some_and(|query| integer(dict(query), "usec") < stamp)
        {
            observe(&session, queries.next().unwrap());
        }
        apply(&mut session, event);
        let close = session.close_usec();
        assert_eq!(
            close,
            integer(dict(&states[index]), "close_usec"),
            "{id} close_usec after event {index}"
        );
        actual_states.push(record! {
            done: Variant::Bool(session.is_done()),
            close_usec: close,
        });
    }
    for query in queries {
        observe(&session, query);
    }
    let execution = session.execution().map_or(Variant::Nil, |actual| {
        let expected = dict(field(expected, "execution"));
        assert_eq!(
            actual.impact_usec,
            integer(expected, "impact_usec"),
            "{id} impact_usec"
        );
        assert_eq!(
            actual.timing_fault.partial_cmp(&0.0),
            number(expected, "timing_fault").partial_cmp(&0.0),
            "{id} fault side"
        );
        execution_snapshot(actual)
    });
    record! {
        states: Variant::Array(Array::new(actual_states)),
        execution: execution,
        views: Variant::Array(Array::new(actual_views)),
    }
}

#[test]
fn every_recorded_swing_matches() {
    let mut golfer_cases = 0;
    let report = tolerance::check(&fixture(), "swing", |case| {
        assert_eq!(field(case, "call").as_str(), Some("session"));
        let id = field(case, "id").as_str().unwrap();
        let input = dict(field(case, "input"));
        if input.get("golfer").and_then(Variant::as_str) == Some("hole_kit") {
            golfer_cases += 1;
        }
        Ok::<_, Infallible>(replay(id, Stream::read(input), dict(field(case, "output"))))
    })
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(report.checked, CASES);
    assert_eq!(golfer_cases, 56);
    assert!(report.skipped.is_empty(), "{report}");
    assert!(report.discrete.is_empty(), "{report}");
    print!("{report}");
    println!("{golfer_cases} hole_kit streams, zero discrete differences");
}

fn at_cadence(stream: &Stream<'_>, end_usec: i64, step: Option<i64>) -> Option<Execution> {
    let mut session = stream.session();
    let mut next = stream.start_usec;
    for event in stream.events.iter().map(dict) {
        if let Some(step) = step {
            let time = integer(event, "usec");
            while next <= time {
                session.advance(next);
                next += step;
            }
        } else if field(event, "kind").as_str() == Some("advance") {
            continue;
        }
        apply(&mut session, event);
    }
    if let Some(step) = step {
        while next <= end_usec {
            session.advance(next);
            next += step;
        }
    }
    // A timeout still needs the caller to report the end of the observation.
    session.advance(end_usec);
    session.execution()
}

#[test]
fn advance_cadence_preserves_every_execution() {
    let fixture = corpus::read(&fixture()).unwrap();
    let cases = items(field(dict(&fixture), "cases"));
    assert_eq!(cases.len(), CASES);
    for case in cases.iter().map(dict) {
        let id = field(case, "id").as_str().unwrap();
        let stream = Stream::read(dict(field(case, "input")));
        let mut recorded = stream.session();
        let mut end_usec = stream.start_usec;
        for event in stream.events.iter().map(dict) {
            apply(&mut recorded, event);
            end_usec = end_usec.max(integer(event, "usec"));
        }
        let expected = recorded.execution();
        assert_eq!(
            at_cadence(&stream, end_usec, None),
            expected,
            "{id} with no intermediate advances"
        );
        assert_eq!(
            at_cadence(&stream, end_usec, Some(1_000)),
            expected,
            "{id} with advances every 1 ms"
        );
    }
    println!(
        "{CASES} executions unchanged with no intermediate advances and with advances every 1 ms"
    );
}
