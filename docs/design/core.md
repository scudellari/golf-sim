# The golf-sim core

golf-sim is a deterministic golf simulation in Rust. This document explains how the core is shaped, the rules that keep it deterministic, how it is checked against measured data, how a person or an agent finds out what happened in a stroke, the formats it reads and writes, and the plan that builds it in small units. The decisions behind it are in [`docs/adr/`](../adr/README.md).

Status: in progress, 2026-11-26. The workspace, its stubs and the determinism lints have landed (C01), and C63 brings them to this revised design. [Units](#units) lists the work.

## What the core does

The core decides every outcome of a round of golf. That covers the ball's flight, bounce and roll, the strike, the course's ground, the day's weather and turf, the lie, the stroke, the Rules of Golf, and the flow of a hole and a round with its record. Nothing outside the core changes an outcome.

Three callers use the same core:

- **A test or an agent** plays through the library or through the `golf` command-line program, with no game engine. Its job is automated testing: regression tests with expected results, playtests, and balance sweeps. It states each swing exactly, or lets a seeded skill profile press the meter.
- **A game engine** draws the round and turns a person's input into the core's commands. The `golf-sim-godot` crate binds the core to Godot 4.7.
- **A replay** applies a saved record, and the core checks every stored result against the one it computes.

The same seed plays the same round on macOS, Linux and Windows. Every record and every repro bundle replays bit for bit, and any stroke can be explained phase by phase.

The core began as a port of a GDScript implementation that runs inside a Godot game. The goal is physics that matches measured golf, not the same bits as that implementation. Each physics unit is accepted against published measurements, and the earlier implementation serves as a coarse check within a tolerance until it is retired. [Checks](#checks) says how.

## Usage

### A regression test in Rust

```rust
use golf_sim::{Action, ClubId, Course, Day, End, Hole, HoleStart, Lie, Mode, StrokeChoice, Swing};

let course = Course::open(Path::new("testdata/courses/test-course"))?;      // checks the manifest's SHA-256
let mut hole = Hole::start(course.clone(), HoleStart::new(1, Day::Constant, 7))?;

let drive = StrokeChoice::stock(ClubId::Driver, hole.observe(Mode::Caddie).aim_point)
	.with_swing(Swing::Exact { power: 1.0, timing_fault: 0.0, strike_m: [0.0, 0.0] });
let step = hole.act(Action::Stroke(drive))?;                      // one deterministic outcome
assert_eq!(step.stroke.end, End::AtRest);
assert!((step.stroke.carry_m - 228.4).abs() < 0.05);
assert_eq!(step.stroke.lie, Lie::Fairway);
std::fs::write("hole.rec", hole.record().to_bytes())?;            // the mode and the seed are in the record
```

An exact swing states the meter's result: the power, the ring's timing fault (early or late, which sets the face and path) and the strike point on the face. The club and the aim come with the stroke choice. The outcome is a function of the course, the day, the seed and those values, so a test can expect it.

### A balance sweep

```rust
let profile = Profile::HCP15;                                    // provisional values, see Units (C20)
for seed in 1..=500 {
	let mut hole = Hole::start(course.clone(), HoleStart::new(5, Day::Constant, seed))?;
	while !hole.prompt().is_done() {
		let action = golfer.decide(&hole);                    // the scripted golfer
		hole.act(action.with_swing(Swing::Skill(profile)))?;   // the profile presses the real meter
	}
	scores.push(hole.score());
}
```

A skill profile presses a real swing meter with seeded errors in a person's units, milliseconds and millimetres. A hard lie narrows the timing window, so the same profile faults more there. The same profile and seed give the same round every time. `golf sweep` does this from the command line and reports the score distribution and where each stroke finished. `Round` has the same calls and adds `Action::PickUp` and the walk between holes.

### A test or an agent over JSON

`golf play` reads one JSON request per line on stdin and writes one reply per line on stdout:

```text
$ golf play --course testdata/courses/test-course --hole 1 --day constant --seed 7 --mode caddie
> {"id":1,"op":"observe"}
< {"id":1,"ok":{"protocol":1,"prompt":{"play":{}},"hole":{"number":1,"par":4},"ball":{"spot":[12.5,301.2,-8.1],"lie":"tee"},
   "to_cup_m":351.7,"strokes":0,"wind":{"felt_mps":3.1,"from_deg":245.0},"advice":{"club":"driver"}}}
> {"id":2,"op":"preview","stroke":{"club":"driver","aim_at":[402.0,-88.5]}}
< {"id":2,"ok":{"carry_m":228.4,"plays_like_m":236.0,"wind_aloft":"more","roll_m":19.2}}
> {"id":3,"op":"act","action":{"stroke":{"club":"driver","aim_at":[402.0,-88.5],"swing":{"exact":{"power":1.0,"timing_fault":0.0,"strike_m":[0.0,0.0]}}}}}
< {"id":3,"ok":{"stroke":{"end":"at_rest","carry_m":224.9,"total_m":246.3,"lie":"fairway"},"ruling":null,"prompt":{"play":{}}}}
> {"id":4,"op":"act","action":{"drop":{"at":[1.0,2.0,3.0]}}}
< {"id":4,"refused":{"not_now":{"prompt":"play"}}}
> {"id":5,"op":"save","path":"runs/hole.rec"}
```

A stroke's swing is `{"exact":{...}}` or `{"profile":"hcp15"}`. `--profile <name>` sets the default for strokes that name none. The operations are `observe`, `legal`, `preview`, `act`, `record` and `save`. Every double crosses the protocol exactly, because `serde_json` with its `float_roundtrip` feature writes the shortest text that reads back to the same bits. A test sent a million random doubles through it and got every one back unchanged. `golf schema` prints the protocol's JSON schema, which is a committed snapshot. Any change to it raises the `protocol` number.

Other commands:

- `golf batch --course <dir> --seeds 1-1000 --profile hcp15 --jobs 8` plays holes or rounds in parallel and writes one JSON line per play, in seed order.
- `golf sweep --course <dir> --holes 5 --days 1-20 --profiles scratch,hcp15 --seeds 1-500 --jobs 8` plays a hole or a round over days, profiles and seeds, and reports the score distribution, fairways, greens, putts, penalties, and where each stroke finished.
- `golf scenario run <file>` plays a scripted hole with exact swings and checks each expected result. `golf scenario bless <file>` writes the expectations from a run, and `golf scenario from-repro <bundle>` turns a failure into a scenario.
- `golf replay <record or bundle> [--stroke N]` applies a record and names the first entry that diverges.
- `golf explain <record or bundle> --stroke N` tells the story of one stroke, phase by phase.
- `golf validate` runs the measured-data targets and prints the residuals.
- `golf stroke`, `golf day`, `golf digests`, `golf probe` and `golf course check` run single pieces for tools and tests.

### A game engine through Godot

```gdscript
var session := GolfSession.open_round(course_dir, setup, profile)   # golf-sim-godot
func _on_command(command: Dictionary) -> void:                      # a stamped key press or mouse move
	_emit(session.handle(command))
func _process(_delta: float) -> void:
	var now := Time.get_ticks_usec()
	_emit(session.advance(now))
	_draw(session.view(now))                                        # a Dictionary of the core's view values
```

`GolfCourse` answers the queries a view makes to place or pick things (`ground_at`, `height_at`, `material_at`, `ray_hit`). It also hands over the payload's drawing data untouched. `GolfSession.traces(n)` and `log_tail(n)` give a feedback bundle the last strokes' traces and the core's log. The binding names no class of any game. It converts the core's own values to Godot values and back, and every call runs on the main thread.

## The shape of the core

### One function applies every decision

A decision is the unit of play: a stroke, a drop, a choice of relief, a provisional ball, going forward, picking up. Each decision becomes a record entry, and one function applies every entry:

```text
person:  stamped key presses -> Console -> Action --\
test:    Action with an exact swing or a profile ----> resolve() -> Entry -> apply() -> Step
replay:  Entry with stored results ------------------------------^   compares, names the first divergence
```

- `Hole::act(action)` resolves an action into an entry: the address from its settings, the stance's footprint, the execution the swing produces, and the gust time. Then it calls `apply`.
- `apply(entry)` plays the stroke or the ruling, moves the clock and the wear, settles the lie, and asks the rules what comes next. With no stored results it stores them. With stored results it compares them exactly and refuses at the first difference.
- The prompt is derived from the rules state on every call and is never stored beside it.
- `observe()`, `legal()`, `preview()` and `prompt()` take `&self` and change nothing. `Hole` holds no interior mutability and no lazy cache, so it is `Send + Sync`, and a sweep can run previews on many threads.

[ADR 0001](../adr/0001-every-decision-goes-through-one-apply.md) records this decision, and [ADR 0005](../adr/0005-tests-and-agents-state-a-swing-or-use-a-seeded-profile.md) amends it for tests and agents.

### Three ways to make a swing

A person's swing goes through a meter: a press to start, a press at the power, a press when the ring closes, and the steering of the strike point. The `swing` crate judges those stamped presses into an `Execution`. A test or an agent gets an `Execution` one of two ways:

- **Exact.** `Swing::Exact { power, timing_fault, strike_m }` states the meter's result, and the core fills in the rest from the swing spec: the ring's close, the backswing, the timing error and the impact stamp. Regression tests use it.
- **A skill profile.** `Swing::Skill(profile)` presses a real swing session with seeded errors, drawn from a `SeedStream` seeded with `Seed::derive(attempt_seed, "hands")`. Balance sweeps use it. The presets are `scratch` and `hcp15`, and their values are provisional until they are calibrated against measured shot dispersion.

Each record entry says which way its execution was made, with an exact swing's values or the profile's, so a record reproduces itself whatever made it. An agent's swing starts 0.25 s after the address starts, and the entry stores that time, so the gust a stroke meets follows from the swing alone. No caller waits at address.

The scripted golfer behind `golf batch` and `golf sweep` decides from what a player would know: the `observe` numbers and the caddie's preview. That is a choice inside that golfer, not a limit of the interface. The library and the program read the whole hole.

### The person's console

A person drives a game with key presses, not decisions. The `Console` turns stamped input into actions. It owns the state that belongs to a screen: the address being built, the swing meter in progress, the relief option under the cursor, the drop spot being picked, the club the caddie handed over, the briefing, and the end of a flight's playback. It schedules the caddie's preview 150 ms after a change settles. Its `view(now)` builds the values a screen draws and changes nothing. A pressed stroke's entry keeps its stamped presses, so a replay can judge the meter again.

The console holds no rule. It only produces actions, so a person and a test reach the same `apply`.

### The world one stroke reads

A `World` is a value that holds the ground, the air, the cup, the clock, the legal areas and the round's moment. A stroke reads one world. The world changes only when `apply` commits a stroke, through a new clock or new wear.

- `Ground` and `Air` are closed enums with a native form for every case: the course's ground, a generated day's ground, the practice ground, a plane, and the stroke's own wrappers. The physics never calls out of the core.
- A generated day is a `RoundPlan` that holds every hourly state. No layer is computed during play.

### Crates

The crates follow the dependency order of the domain, so the compiler enforces which layer may use which.

| Crate | Owns | Depends on |
| --- | --- | --- |
| `golf-sim-base` | `fm` (every transcendental), engine-order vectors, plane geometry, seeds, the Variant codec, the bundle container, digests, the stroke trace, the log's span names | nothing |
| `golf-sim-physics` | Flight, bounce, roll, the cup, obstacles, the strike, tunables, the sampler traits, the plane ground, constant air, the zone map | base |
| `golf-sim-course` | The course bundle reader, grids, materials, regions, obstacles, the legal map, `ray_hit` | base, physics |
| `golf-sim-conditions` | The world, the samplers, wear, lies, the round plan, the generator, course analysis | base, physics, course |
| `golf-sim-swing` | The meter judgment, exact swings and skill profiles | base |
| `golf-sim-stroke` | The bag, the address, the delivery, a stroke, a preview, a drop | base, physics, course, conditions, swing |
| `golf-sim-rules` | The hole's rules state, rulings, relief | base, physics, course, stroke |
| `golf-sim` | `Hole`, `Round`, `Action`, records, observation, the console, the range | every crate above |
| `golf-sim-validate` | The measured-data targets, the licence ledger and the residual report | nothing |
| `golf-sim-cli` | The `golf` program, repro bundles, `explain`, sweeps and scenarios | golf-sim, golf-sim-validate, golf-sim-testkit (for `golf corpus`) |
| `golf-sim-godot` | The Godot binding, a `cdylib` | golf-sim |
| `golf-sim-testkit` | The determinism suite, the tolerance comparisons, the fixture loaders and the earlier implementation's record decoders, for tests only | golf-sim |

`golf-sim`, `golf-sim-cli` and `golf-sim-godot` are the entry points. The other crates are internal and are not published on their own. A consumer depends on `golf-sim`, and a check in the consuming game fails if it names an internal crate.

## Finding out what happened

A person who plays the game, a programmer and an agent each need to find out why a stroke did what it did. The core makes that a matter of running a command, not of guessing. [ADR 0006](../adr/0006-every-result-can-be-replayed-and-explained.md) records the decision.

### Every record reproduces itself

A record carries everything that reproduces it: the seeds, the golf-sim release and commit, every model version, the course revision, the conditions request for a generated day, and each decision with the swing that made it (the stamped presses, an exact swing, or a profile with its values). `golf replay <record>` plays it again on the same build and requires the same bits, and `--stroke N` stops at stroke N. Because the determinism rules hold on every platform, a record from one machine replays on the others.

### Repro bundles

A repro bundle is a folder that one command replays:

| File | Holds |
| --- | --- |
| `bundle.json` | `format`, the `engine` that wrote it, the time, a note, the course (`id` and `revision`), the focus (the hole and the stroke), and the list of files present. A reader keeps keys it does not know. |
| `record.bin` | The stroke repro, hole record or round record, as its codec writes it. |
| `traces.json` | Optional. The stroke traces of the last strokes, as below. |
| `log.jsonl` | Optional. The log's tail, one JSON event per line. |

A failing scenario and a failing `golf batch` play each save a bundle under `repros/`. `golf replay` and `golf explain` find the course by `--course <dir>` or in the folders of `GOLF_COURSES`, and refuse a course whose revision differs from the bundle's. A game's playtest feedback is the same format with more files, such as a screenshot and the camera and settings, which the core ignores. `golf replay <bundle>` replays to the focus. `golf explain <bundle> --stroke N` explains a stroke. `golf scenario from-repro <bundle>` writes a scenario that expects the bundle's results, so a fixed bug becomes a regression test with one command.

### Stroke traces

`golf explain` plays a stroke with a `Trace` and prints a plain-language account of each phase, then the trace as JSON. An account looks like this (the values and tunable names are illustrative):

```text
$ golf explain repros/lip-out --stroke 3
delivery   7-iron, 83.6 mph club speed, -4.0 deg attack, 24.0 deg delivered loft (Stock), strike 6 mm low
impact     107.3 mph ball speed, 16.8 deg launch, 7,077 rpm (impact model 0.8.0)
flight     apex 29.8 m at 3.1 s; landed 147.3 yd out at 41 m/s, 48 deg (drag and lift: tunables AERO_*)
bounce 1   on firm fairway (firmness 0.72): restitution 0.41 normal, spin 6,200 rpm in, 3,900 rpm out; bounced 3.1 m
roll       checked because 6,200 rpm exceeded the release threshold (tunable SPIN_CHECK_RPM); rolled 2.4 m; stopped: speed below 0.05 m/s
lie        fairway, sitting up 4 mm (one seeded draw, seed lie/3)
```

The physics functions take `&mut impl Probe`, and `Probe` has `const ENABLED: bool`. A `Trace` records every phase: the delivery, the impact and the launch; the flight, sampled; each bounce with its incidence, surface, firmness, restitution and spin in and out; the skid, the roll and the reason it stopped; the cup; the lie; and each ruling with its reason. Each value names the tunable, the turf or course value, or the seed draw it came from. Work done only for the trace runs inside `if P::ENABLED` or in a closure. `NoProbe` is a zero-sized type with `ENABLED = false` and empty methods, so a stroke without a trace compiles to the same code. A benchmark holds the cost of `NoProbe` to at most 1%, and a test requires the same bits with the trace on and off. Unit C56 writes the JSON schema to `docs/design/trace.md`.

### Logging

The core logs with the `tracing` crate. Spans cover the round, the hole, the stroke and each physics phase, and carry the seed, the hole, the stroke and the club. `golf --log <file> --log-level <level>` writes one JSON event per line. With no subscriber, a disabled event costs one check. A log event only reads values. The determinism suite runs once with logging at the `trace` level and once with logging off, and requires the same digests. The Godot binding forwards the core's events into Godot's log by level and keeps a tail for a feedback bundle.

### Breakpoints

Debug builds keep their symbols, and the `debugging` profile adds full debug information to an optimized build. `docs/how-to/debug.md` (unit C60) shows how to set breakpoints in VS Code with CodeLLDB and in lldb, how to debug `golf replay` (`just debug-replay <bundle>` builds the profile and starts the replay under lldb), and how to attach to a running Godot that loaded the binding's debug library. The core keeps its hot paths free of macros that hide control flow from a debugger.

## The determinism rules

These rules make a test run on one machine stand for every platform. Each rule has a check that fails the build when it is broken.

| Rule | Check |
| --- | --- |
| One pure-Rust maths library computes every transcendental: the `libm` crate with its `force-soft-floats` feature, called only through `golf_sim_base::fm`. The core never calls the platform's maths library. | Clippy's `disallowed-methods` bans every `f32` and `f64` transcendental method (`sin`, `cos`, `tan`, `atan2`, `exp`, `ln`, `log10`, `powf`, `powi`, `sqrt`, `hypot`, `sin_cos` and the rest) in every crate. `fm` calls the `libm` crate's functions, not std's. |
| No fused multiply-add, written or produced by a compiler flag. | `mul_add` is a disallowed method. The workspace sets no `target-cpu` or `target-feature`. A test computes `a * b + c` for inputs where a fused result differs and requires the unfused one. |
| No hash-map order reaches an outcome. Iterate ordered maps, or sort before iterating. | `disallowed-types` bans `std::collections::HashMap` and `HashSet` in every crate. |
| Parallel work reduces in a fixed order. The core starts no thread. | `disallowed-methods` bans `std::thread::spawn` and `std::thread::scope` outside `golf-sim-cli` and `golf-sim-godot`. A test fails when a core crate's dependency tree holds a thread pool. `golf batch` and `golf sweep` run independent plays in parallel and write them in seed order. The binding may move a job to its own thread, but the job is a whole stroke or preview, so no result depends on which thread ran it. |
| The core computes in 64-bit floats. Single precision appears only where a format stores it, and the codec widens it on read. | A test scans the core crates for `f32` outside the codecs. |
| Loading a course does not depend on path separators or line endings. | `Course::open` joins paths with `Path::join`, resolves manifest names by their `/` components, parses JSON, and hashes bytes, never text. `.gitattributes` keeps text at LF and marks fixtures binary. A test opens a bundle through a path with `.` and `..` parts and a manifest rewritten with CRLF, and requires the same revision and the same course. |
| Watching never changes a result: not a trace, not a log event. | A stroke gives the same bits with a `Trace` and with `NoProbe`. The determinism suite gives the same digests with logging at `trace` and with logging off. |

[ADR 0002](../adr/0002-one-pure-rust-maths-library-on-every-platform.md) records the maths decision. The pure maths library and Apple's library disagree by one unit in the last place on 1% to 18% of inputs, depending on the function. So a result computed with the platform's library would differ between machines, and these rules exist to stop that.

### When the platforms are checked

- **Every change, on the developer's Mac.** `cargo test --workspace` runs the unit tests and the determinism suite. The rules above remove the known causes of a platform difference, so a pass on one machine is strong evidence for the others.
- **Every push and pull request, on Linux.** `.github/workflows/ci.yml` runs `cargo test --workspace` and clippy with the determinism lints on `ubuntu-latest`, with a cache. It catches a broken build or a broken rule within minutes.
- **Each release, and on demand.** The [release workflow](#build-and-release) runs the same tests and the determinism suite on macOS, Linux and Windows, and compares the three platforms' digests. Run it by hand (`workflow_dispatch`) before a tag. The build plan runs it at the end of every phase, so a platform difference is found in the phase that caused it.

### The determinism suite

The suite plays a fixed set of strokes and seeded rounds on the synthetic test course and computes the SHA-256 of each result's Variant bytes. Each layer keeps its items in `crates/golf-sim-testkit/src/items/<layer>.rs` and their expected digests in `testdata/determinism/<layer>.json`. A change that moves a digest fails the suite until the change raises the version it affects and the layer is stored again with `cargo run -p golf-sim-testkit -- bless <layer>`.

## Checks

The core is checked three ways, and each answers a different question. [ADR 0004](../adr/0004-checked-against-measured-golf-not-the-earlier-implementation-s-bits.md) records why.

| Check | Question it answers | Where |
| --- | --- | --- |
| Validation against measured data | Does the ball do what a real ball does? | `golf validate`, `golf-sim-validate` |
| The earlier implementation within a tolerance | Did the port slip? | `golf corpus check`, `testdata/<layer>/` |
| The determinism suite and the records | Does the same input give the same bits everywhere, every time? | `cargo test`, `golf replay` |

### Validation against measured data

The acceptance standard for the physics is published measurement. Each physics unit is done when its targets pass:

| Layer | Targets |
| --- | --- |
| Turf bounce | Biber et al. (2023), 693 natural-turf trials, and Haake (1989) high-backspin impacts |
| Run-out and greens | The USGA and R&A bounce-and-roll trials, stimpmeter exit speed, Penner's crater model |
| The stroke | Stock carries per club against tour and USGA/R&A launch-condition data |

A target holds the inputs (a launch, a club delivery, an impact on turf, a run-out, a stimp reading), the measured values, the source's own tolerance where it states one, and the earlier implementation's residual. A target passes when the core is within the source's tolerance, or no further from the measurement than the earlier implementation was, plus the layer's tolerance. So the port can never make a residual worse, and a model that closes a known gap shows it.

A target whose dataset the ledger marks `fetch` and whose data is not present is skipped and listed, so the suite runs offline. `golf validate` prints the residual report: for each source, club and shot type, the prediction, the measurement and the residual. `--json` writes it, so two models can be compared before and after a change. The data lives in `testdata/validation/targets/`, one file per source, with its citation.

**The licence ledger.** `testdata/validation/targets/LICENCES.toml` lists every dataset with its terms and how the core uses it. A test fails when a data file has no entry.

- Openly licensed data goes in whole with its attribution. Biber's supplement is CC BY 4.0.
- A dataset whose licence is unclear is listed as `fetch`, with its URL. Its values stay out of the repository, and the calibration fetches it when it runs.

**Calibration.** `calibration/fit.py` and its helpers fit the marked blocks of the tunables (`BEGIN FIT` to `END FIT` in `tunables.rs`) to the targets. `golf fit impact`, `golf fit runout` and `golf fit parity` give the script the model's outputs. `fit.py` without `--write` checks that the committed blocks still come out of the fit.

### The earlier implementation, within a tolerance

Until the GDScript implementation is retired, it is a coarse oracle. Tools in the consuming game record its inputs and outputs layer by layer as fixture files: the magic `GSCORP01`, a u32 length, and Variant bytes of `{format, layer, oracle: {versions, commit}, cases: [{id, call, input, output}]}`. Fixtures on the synthetic test course and on flat ground live in `testdata/<layer>/<set>/`, and the first unit that compares against a set commits it.

`golf corpus check <fixture> --layer <layer>` runs each case through the core and compares it within that layer's tolerance in `testdata/tolerances.toml`. It reports the path and the size of the first difference beyond it, such as `cases[12].output.events[3].time_s: 0.0021 s`. `golf corpus drift <fixture>` reports the largest absolute and relative difference of each numeric field, and every case whose discrete result differs: an end reason, a surface, a prompt, a holed ball.

The earlier implementation computes vectors in single precision and takes its transcendentals from the platform's maths library, so the core cannot match its bits and does not try. The tolerances are wide enough for that difference and narrow enough to catch a porting slip, such as reading a value before it is resampled:

| Layer | Numbers | Discrete results that may differ |
| --- | --- | --- |
| Maths built-ins | 4 ULP for a scalar, 1e-6 relative for a vector operation | none |
| Seeds | Derivations and streams exact, Gaussians 1e-12 relative | none |
| Variant bytes, records, course decoding | Exact | none |
| Physics values, flight step, contact, impact | 1e-5 relative, or 1e-6 in SI units near zero | none |
| Whole strokes and the tools built on them | Rest and samples within 0.05 m, carry within 0.1 m, speeds within 1e-4 relative | 0.5% of cases, each listed |
| Rules, relief, the console | Positions within 1 mm, times within 1 microsecond | none, except after a listed stroke difference |
| The hole and the round | Compared as rulings, prompts and stroke counts | 0.5% of holes, each listed |
| The generator and the analysis | 1e-4 relative | 1% of seeds, each listed |

**Single-precision reference data.** A value the earlier implementation passed through a vector or a packed float array was rounded to single precision, so its error grows with the size of the numbers it came from. A tolerance against such data is written in single-precision steps of the operands' magnitude: a number may differ by k × 2^-23 × s, where s is max(1, the largest magnitude among the operands and the result), never by one fixed absolute bound. The maths tests use 8 steps for a vector operation and the geometry tests 4 steps of the largest coordinate. `testdata/tolerances.toml` states the rule as `f32_steps = k` on a layer or field row, with `scale_from` naming the input paths whose magnitude enters s, and a number passes when it is within any of its row's bounds.

These are starting values. A unit may tighten its layer's row when its drift report shows room. Widening a row needs a drift report that shows the difference comes from precision, posted with the change.

A discrete difference comes from a threshold: a putt that dies on the lip in one implementation drops in the other. The comparison of a hole stops at the first stroke whose discrete result differs, and the test lists it. Traces of the console compare every stamp, discrete fields exactly, until such a stroke.

### Improving the physics

Where the earlier implementation is known to be off, the core may fix it: for example the 4- through 9-iron Stock carries that fall 5.5 to 9.9 yd short, or the lift that saturates at high spin. A model change lands this way:

1. Raise the physics version.
2. Show the scorecard before and after with `golf score --against <before.json>`, where the before file comes from the parent commit. No target may fail, the rows the change is for must improve, and no row may get worse beyond its tolerance.
3. List each fixture case that now leaves its tolerance in `testdata/departures.toml`, with the change that moved it. The corpus check skips listed cases.
4. Store the determinism suite's affected layers again.

New data and new models arrive the same way. A new source is a target file and a ledger entry. A new model of a phase, such as a different lift curve or a turf model with speed-dependent resistance, sits behind the same physics facade, and the residual report decides whether it replaces the current one.

**The scorecard.** Every target names its sub-model (flight, impact, turf bounce, run-out and greens, the cup, the stroke) and its role: `fit` targets calibrate the model and `held-out` targets only score it. A scorecard row is a source, a sub-model and a role, with the licence verdict from the ledger, the tolerance and the residuals. A target with neither a tolerance nor an earlier prediction is reported and never failed, and a `local` dataset is skipped when absent, like a `fetch` one. A sub-model's score is the mean of its rows' residuals as a share of their tolerances, given separately for fit and held-out rows. `golf score` prints it, and the calibration fits only `fit` rows.

The first model changes of this kind are planned as units: the flagstick's rim response, cup capture on slopes, green bounce across the iron range, and lift capped near a spin ratio of 0.3. Each lands after the layer it changes has been ported and after the scorecard, and they land one after another because they share the tunables and the departures. Once a change lands, every later comparison with the earlier implementation carries its departures, so a later layer lists the cases the change moved rather than failing on them.

## Formats

### The course bundle

A course is a folder that holds `manifest.json` and `course.payload`. The bundle is produced by a course baker outside this repository. The core only reads it.

- `manifest.json` names the format (`format_version` 1), the bake version, the course id, the revision, and each file with its SHA-256.
- `course.payload` is a Godot compressed file: the magic `GCPF`, a u32 mode (2 is zstd), a u32 block size, a u32 total size, one u32 compressed size per block, then the blocks. Each block is a zstd frame of at most one block size.
- The decompressed bytes are one Godot Variant, a dictionary. The course's revision is the SHA-256 of those bytes.

`Course::open` refuses a format version it does not know, a revision that differs from the manifest, and a missing or mistyped key, which it names by its path. A spike read two real bundles of 5.9 MB and 76.4 MB this way. Both revisions matched, and re-encoding the decoded Variant gave back every byte.

### Variant bytes

The core reads and writes Godot 4.7's binary Variant encoding exactly. A value starts with a u32 header whose low byte is the type. A float is 4 bytes when it survives a round trip through `f32`, otherwise 8 bytes with header bit 16 set. An int is 4 bytes when it fits in `i32`, otherwise 8 bytes with bit 16 set. A string is a u32 length, its UTF-8 bytes, and padding to 4 bytes. A dictionary keeps insertion order, and the core's `Dict` is an ordered list of pairs. The types in use are nil, bool, int, float, String, Vector2, Vector2i, Vector3, Transform3D, Array, Dictionary, PackedByteArray, PackedFloat32Array, PackedFloat64Array, PackedStringArray, PackedVector2Array and PackedVector3Array. [ADR 0003](../adr/0003-godot-variant-bytes-for-courses-records-and-digests.md) records why the core uses this format.

### Records

A hole record is the magic `LLHOLE01`, a u32 length, and a Variant dictionary: `{format, versions, course_revision, setup, settings, driver, entries}`. A round record is `LLROUND1`, a u32 length, and `{format, versions, course_id, course_name, course_revision, request, generator, tees, order, pick_up_rule, driver, holes}`, where each hole holds its own record's bytes. The core writes hole format 5 and round format 3, with every position in 64-bit floats. The formats may change freely until the first release that a game ships. No earlier save is kept or converted.

- `versions` names the golf-sim release, the commit (from `GOLF_SIM_COMMIT` at build time, `dev` when unset) and every model version.
- `driver` says what played the record: `{kind: "console"}`, or `{kind: "agent", tool: "golf sweep"}`.
- Each stroke entry's `swing` says how its execution was made: `{mode: "pressed", presses: [...]}`, `{mode: "exact", power, timing_fault, strike_m}`, or `{mode: "skill", profile: {name, values}}`. An agent's entry also stores `address_usec`, the time from the address to the swing.
- With the seed in `setup`, the conditions `request` and the course revision, a record names everything that reproduces it.

A reader keeps keys it does not know in `driver` and `swing`, so a later field, such as who played, needs no new format. The testkit alone decodes the earlier implementation's hole format 4 and round format 2, for the comparisons.

### Digests and fingerprints

A digest is the SHA-256 of a value's Variant bytes, where an object becomes a dictionary of its fields in declaration order. A round plan's fingerprint is the digest of its layers. Its digest adds the request, the generator version, the course revision, and the analysis and physics versions. A request that carries a fingerprint generates that round or an error.

## Build and release

### Building

The workspace needs Rust 1.94 or later. `cargo build` and `cargo test` work offline once the crates are fetched. To build where the network is closed, run `just vendor <dir>` with the network once and point Cargo at the folder with the `[source]` lines it prints. Put those lines in an untracked `.cargo/config.toml`, never in the repository, and build with `--offline`. Run `just vendor` again whenever `Cargo.lock` changes. [CONTRIBUTING.md](../../CONTRIBUTING.md) has the steps.

### The workflows

Two workflows run on GitHub's hosted runners.

- **`.github/workflows/ci.yml`** runs on every push to `main` and every pull request, on `ubuntu-latest` only: `cargo test --workspace` with the determinism suite, and clippy with the determinism lints. It caches the registry and the target folder.
- **`.github/workflows/release.yml`** builds and checks the core on three platforms. It runs only when a tag matching `v*` is pushed, when a GitHub release is published, or when someone starts it by hand.

| Platform | Runner | Target |
| --- | --- | --- |
| macOS | `macos-15` | `aarch64-apple-darwin` |
| Linux | `ubuntu-latest` | `x86_64-unknown-linux-gnu`, built with `cargo zigbuild` against glibc 2.28 |
| Windows | `windows-latest` | `x86_64-pc-windows-msvc` |

On each runner the release workflow runs `cargo test --workspace` with the determinism suite, then builds the release artifacts: the `golf` program and the `golf-sim-godot` library. A final job compares the three platforms' determinism digests and fails on any difference. A tagged run attaches the artifacts and a `SHA256SUMS` file to the GitHub release.

The glibc floor costs nothing in determinism, because the core takes no maths from glibc. A macOS x86_64 or universal library is left for the first macOS export that needs one. It would be one more target on the same runner.

## Units

The core is built in small units. Each unit lands on its own on `main` with its tests, and each has a check that proves it done. Units in the same track touch different files, so they run at the same time. Every unit that writes code here is marked **codex**: it is fully specified, checked by tests, and runs headless and offline. The one unit marked **claude** runs the workflows on GitHub and cuts the first release.

Every unit follows the determinism rules and builds offline. A porting unit's issue names the files, the fixtures it compares against, the measured-data targets it must pass, and the commands that prove it. A unit that needs data from the earlier implementation waits for it, and the table names that wait as reference data from the consuming game.

<!-- units:begin -->
Sizes are estimates in lines of Rust, tests included. Some units also wait for reference data that the consuming game produces. Units whose dependencies are met run at the same time.

### Phase 0. Foundation

| Unit | Title | Track | Label | Depends on | Size |
| --- | --- | --- | --- | --- | ---: |
| [C01](https://github.com/scudellari/golf-sim/issues/1) | Workspace skeleton, shared types, determinism lints and offline builds | Shared base | codex | nothing | ~500 |
| [C53](https://github.com/scudellari/golf-sim/issues/52) | The validation suite: measured targets, the licence ledger, the residual report and the scorecard | Shared base | codex | C48, reference data from the consuming game | ~420 |
| [C02](https://github.com/scudellari/golf-sim/issues/2) | Maths: `fm`, engine-order vectors and the engine's built-ins | Maths and determinism | codex | C01, reference data from the consuming game, C05 | ~550 |
| [C03](https://github.com/scudellari/golf-sim/issues/3) | Plane geometry: point in polygon, nearest point, segment crossing, cell-run union | Maths and determinism | codex | C01, reference data from the consuming game, C05 | ~300 |
| [C04](https://github.com/scudellari/golf-sim/issues/4) | Seeds: `Seed::derive` and `SeedStream` | Maths and determinism | codex | C01, reference data from the consuming game, C05, C02 | ~180 |
| [C05](https://github.com/scudellari/golf-sim/issues/5) | Variant codec, ordered dictionary, bundle container and digests | Variant, bundle and records | codex | C01, reference data from the consuming game | ~550 |
| [C06](https://github.com/scudellari/golf-sim/issues/6) | The determinism suite | Shared base | codex | C02, C04, C05 | ~250 |
| [C48](https://github.com/scudellari/golf-sim/issues/7) | The tolerance table, the drift report and `golf corpus` | Shared base | codex | C05, C06, C63 | ~350 |
| [C07](https://github.com/scudellari/golf-sim/issues/8) | Release workflow: tests, determinism and artifacts on macOS, Linux and Windows | Release | codex | C06, C48 | ~200 |
| [C63](https://github.com/scudellari/golf-sim/issues/53) | Revise the skeleton: no reference feature, new pins, new stubs and a library for the program | Shared base | codex | C01 | ~200 |
| [C52](https://github.com/scudellari/golf-sim/issues/54) | A cheap check on every push: Linux tests and clippy | Release | codex | C63 | ~60 |
| [C62](https://github.com/scudellari/golf-sim/issues/55) | Run both workflows on GitHub and cut v0.1.0 | Release | claude | C07, C52 | n/a |
| [C56](https://github.com/scudellari/golf-sim/issues/56) | The stroke trace: a recorder that costs nothing when off | Shared base | codex | C63 | ~250 |
| [C57](https://github.com/scudellari/golf-sim/issues/57) | Logging: `tracing` spans, levels and JSON files | Shared base | codex | C06, C56 | ~200 |

### Phase 1. Strokes on a constant day

| Unit | Title | Track | Label | Depends on | Size |
| --- | --- | --- | --- | --- | ---: |
| [C64](https://github.com/scudellari/golf-sim/issues/63) | Single-precision steps in the tolerance table | Shared base | codex | C48 | ~150 |
| [C69](https://github.com/scudellari/golf-sim/issues/68) | Move the workflows' actions off the Node.js 20 runtime | Release | codex | C62 | ~40 |
| [C70](https://github.com/scudellari/golf-sim/issues/69) | Keep the testkit's embedded tables inside its crate | Shared base | codex | C64 | ~120 |
| [C08](https://github.com/scudellari/golf-sim/issues/9) | Course data: typed parse and grids | Course | codex | C05, reference data from the consuming game, C48, C64 | ~550 |
| [C10](https://github.com/scudellari/golf-sim/issues/10) | Physics values, sampler traits, the plane ground and constant air | Physics solver | codex | C02, C03, reference data from the consuming game, C48, C56, C57, C64 | ~550 |
| [C09](https://github.com/scudellari/golf-sim/issues/11) | Course obstacles, zone map, legal map and the stance footprint | Course | codex | C08, C03, C10 | ~550 |
| [C11](https://github.com/scudellari/golf-sim/issues/12) | Flight: atmosphere, aerodynamics and the flight step | Physics solver | codex | C10, C53 | ~300 |
| [C12](https://github.com/scudellari/golf-sim/issues/13) | Ground contact: bounce, roll, the cup and solid sweep | Physics solver | codex | C10, C53 | ~400 |
| [C13](https://github.com/scudellari/golf-sim/issues/14) | The stroke integrator | Physics solver | codex | C11, C12 | ~800 |
| [C14](https://github.com/scudellari/golf-sim/issues/15) | Club impact and strike contact | Physics solver | codex | C10, C53 | ~550 |
| [C15](https://github.com/scudellari/golf-sim/issues/16) | The physics facade | Physics solver | codex | C13, C14 | ~300 |
| [C16](https://github.com/scudellari/golf-sim/issues/17) | The world on a constant day | World and stroke | codex | C09, C15, reference data from the consuming game | ~550 |
| [C17](https://github.com/scudellari/golf-sim/issues/18) | Stroke setup: address, bag, tuning and delivery | World and stroke | codex | C16 | ~600 |
| [C18](https://github.com/scudellari/golf-sim/issues/19) | The stroke facade, `golf stroke` and `golf course check` | World and stroke | codex | C17 | ~500 |
| [C54](https://github.com/scudellari/golf-sim/issues/58) | `golf validate`, `golf score`, `golf fit` and the calibration script | Tools | codex | C18, C53 | ~560 |
| [C58](https://github.com/scudellari/golf-sim/issues/59) | `golf explain` and stroke repros | Tools | codex | C54 | ~450 |
| [C65](https://github.com/scudellari/golf-sim/issues/64) | The flagstick's rim response | Physics solver | codex | C12, C54, C58, C70 | ~250 |
| [C66](https://github.com/scudellari/golf-sim/issues/65) | Cup capture on slopes | Physics solver | codex | C12, C54, C65 | ~200 |
| [C67](https://github.com/scudellari/golf-sim/issues/66) | Green bounce across the iron range | Physics solver | codex | C12, C54, C66 | ~350 |
| [C68](https://github.com/scudellari/golf-sim/issues/67) | Lift capped near spin ratio 0.3, then a refit | Physics solver | codex | C11, C54, C67 | ~300 |

### Phase 2. A hole on a constant day

| Unit | Title | Track | Label | Depends on | Size |
| --- | --- | --- | --- | --- | ---: |
| [C19](https://github.com/scudellari/golf-sim/issues/20) | The swing meter's judgment | Swing and rules | codex | C02, C04, reference data from the consuming game, C48, C64 | ~350 |
| [C20](https://github.com/scudellari/golf-sim/issues/21) | Swings: an exact swing and seeded skill profiles | Swing and rules | codex | C19 | ~300 |
| [C21](https://github.com/scudellari/golf-sim/issues/22) | Rules: rulings, prompts and the provisional ball | Swing and rules | codex | C18, C09, reference data from the consuming game | ~450 |
| [C22](https://github.com/scudellari/golf-sim/issues/23) | Relief: areas, complete relief, abnormal conditions, drops and placement | Swing and rules | codex | C21, C03 | ~450 |
| [C23](https://github.com/scudellari/golf-sim/issues/24) | Hole records and the hole's setup | Variant, bundle and records | codex | C05, C17, C19, C16, reference data from the consuming game | ~450 |
| [C51](https://github.com/scudellari/golf-sim/issues/25) | The hole's relief ground | Hole, testing and CLI | codex | C22, C23 | ~250 |
| [C24](https://github.com/scudellari/golf-sim/issues/26) | The hole: actions, entries, `apply` and replay | Hole, testing and CLI | codex | C18, C20, C51 | ~450 |
| [C25](https://github.com/scudellari/golf-sim/issues/27) | The hole: prompts, legal actions, previews and the caddie's advice | Hole, testing and CLI | codex | C24 | ~350 |
| [C26](https://github.com/scudellari/golf-sim/issues/28) | Observation: the HUD's numbers | Hole, testing and CLI | codex | C25 | ~400 |
| [C27](https://github.com/scudellari/golf-sim/issues/29) | `golf play`, `golf replay` and `golf schema` | Hole, testing and CLI | codex | C26, C58 | ~500 |
| [C28](https://github.com/scudellari/golf-sim/issues/30) | `golf batch` and the scripted golfer | Hole, testing and CLI | codex | C27 | ~350 |
| [C55](https://github.com/scudellari/golf-sim/issues/60) | `golf sweep` and `golf scenario`: balance reports and expected results | Hole, testing and CLI | codex | C28, C58 | ~450 |
| [C59](https://github.com/scudellari/golf-sim/issues/61) | Repro bundles: replay a failure, explain it, and turn it into a test | Hole, testing and CLI | codex | C55, C58, C68 | ~400 |
| [C60](https://github.com/scudellari/golf-sim/issues/62) | Debugging guide: breakpoints in the program and in Godot | Hole, testing and CLI | codex | C59 | ~150 |

### Phase 3. The person's console, headless

| Unit | Title | Track | Label | Depends on | Size |
| --- | --- | --- | --- | --- | ---: |
| [C29](https://github.com/scudellari/golf-sim/issues/31) | Console: building the address and swinging | Console | codex | C26, reference data from the consuming game | ~450 |
| [C30](https://github.com/scudellari/golf-sim/issues/32) | Console: watching, the hole's screens and the view | Console | codex | C29 | ~450 |
| [C31](https://github.com/scudellari/golf-sim/issues/33) | The range | Console | codex | C29 | ~450 |

### Phase 4. A round on an exported day

| Unit | Title | Track | Label | Depends on | Size |
| --- | --- | --- | --- | --- | ---: |
| [C47](https://github.com/scudellari/golf-sim/issues/34) | The course analysis value and its cache bytes | Round ground, air and round | codex | C05, C08, reference data from the consuming game | ~400 |
| [C32](https://github.com/scudellari/golf-sim/issues/35) | Round layers, the request and the plan | Round ground, air and round | codex | C47, C16 | ~550 |
| [C33](https://github.com/scudellari/golf-sim/issues/36) | Round ground and air, the moment and `World::at` | Round ground, air and round | codex | C32, reference data from the consuming game | ~450 |
| [C34](https://github.com/scudellari/golf-sim/issues/37) | The round: holes in order, the walk, the card and the round record | Round ground, air and round | codex | C33, C24, C59 | ~550 |
| [C49](https://github.com/scudellari/golf-sim/issues/38) | Console: the round's stages | Console | codex | C34, C30 | ~300 |

### Phase 5. The day's generator and course analysis

| Unit | Title | Track | Label | Depends on | Size |
| --- | --- | --- | --- | --- | ---: |
| [C35](https://github.com/scudellari/golf-sim/issues/39) | The generator's engine | Generator and analysis | codex | C32, C04, reference data from the consuming game | ~600 |
| [C36](https://github.com/scudellari/golf-sim/issues/40) | Passes: calendar, weather pattern, history and timeline | Generator and analysis | codex | C35 | ~550 |
| [C37](https://github.com/scudellari/golf-sim/issues/41) | Passes: maintenance, rough, wear, moisture, firmness and green speed | Generator and analysis | codex | C35 | ~550 |
| [C38](https://github.com/scudellari/golf-sim/issues/42) | Passes: temporary water, ground under repair, bunker sand, frost and pins | Generator and analysis | codex | C35 | ~500 |
| [C39](https://github.com/scudellari/golf-sim/issues/43) | The playability checks and the passes table | Generator and analysis | codex | C35 | ~400 |
| [C40](https://github.com/scudellari/golf-sim/issues/44) | Terrain analysis | Generator and analysis | codex | C47, reference data from the consuming game | ~450 |
| [C41](https://github.com/scudellari/golf-sim/issues/45) | Course analysis: the builder and the green surfaces | Generator and analysis | codex | C40, C15, C16 | ~550 |
| [C42](https://github.com/scudellari/golf-sim/issues/46) | `Conditions::generate`, `golf day` and any seed for the agent | Generator and analysis | codex | C36, C37, C38, C39, C41, C34 | ~350 |

### Phase 6. Tools on the command line

| Unit | Title | Track | Label | Depends on | Size |
| --- | --- | --- | --- | --- | ---: |
| [C43](https://github.com/scudellari/golf-sim/issues/47) | `golf probe`, `golf reach`, `golf bag` and the range tables | Tools | codex | C18, C42, reference data from the consuming game | ~500 |
| [C44](https://github.com/scudellari/golf-sim/issues/48) | `golf strategy` and `golf play-course` | Tools | codex | C42, C28, reference data from the consuming game | ~400 |

### Phase 7. The game on the core

| Unit | Title | Track | Label | Depends on | Size |
| --- | --- | --- | --- | --- | ---: |
| [C45](https://github.com/scudellari/golf-sim/issues/50) | The Godot binding | Godot binding | codex | C49, C31, C42 | ~700 |
<!-- units:end -->

## Later: agents as competing players

Today an agent plays to test the game. A later version could let an agent play the game as a competitor, beside people. That would add:

- **An information view.** A filter in front of `observe` that shows an agent only what a person in the same information mode sees, with no seed and no coming gust, and a matching limit on the program's other reads.
- **Patience at address.** `Action::Wait`, which lets an agent spend address time to meet a different gust, as a patient person can.
- **Fair hands.** A rule that a competing agent swings only through a skill profile, never an exact swing, so a hard lie costs it what it costs a person.
- **Who played.** A player's identity in each record, and leaderboards that keep agents' rounds apart from people's.

Today's design leaves room for each, and nothing above blocks them:

- `observe` is one function over `&Hole`, so an information view can wrap it without touching the hole.
- Every decision goes through one `apply`, so a competing agent reaches the same rules and records as a person.
- Each record states the mode and the seed behind every swing, and its `driver` and `swing` dictionaries keep unknown keys, so a player field needs no new format.
- Each agent entry stores its address time, so a later wait changes no earlier record's replay.
