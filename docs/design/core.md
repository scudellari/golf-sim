# The golf-sim core

golf-sim is a deterministic golf simulation in Rust. This document explains how the core is shaped, the rules that keep it deterministic, the formats it reads and writes, and the plan that builds it in small units. The decisions behind it are in [`docs/adr/`](../adr/README.md).

Status: design, 2026-11-26. No code has landed yet. [Units](#units) lists the work.

## What the core does

The core decides every outcome of a round of golf. That covers the ball's flight, bounce and roll, the strike, the course's ground, the day's weather and turf, the lie, the stroke, the Rules of Golf, and the flow of a hole and a round with its record. Nothing outside the core changes an outcome.

Three callers use the same core:

- **An agent** plays through the library or through the `golf` command-line program, with no game engine.
- **A game engine** draws the round and turns a person's input into the core's commands. The `golf-sim-godot` crate binds the core to Godot 4.7.
- **A replay** applies a saved record, and the core checks every stored result against the one it computes.

The same seed plays the same round on macOS, Linux and Windows.

The core began as a port of a GDScript implementation that runs inside a Godot game. Until the port is complete, that implementation is the reference: each unit proves its output against data the reference produced. [Proof against the reference](#proof-against-the-reference) says how.

## Usage

### An agent in Rust

```rust
use golf_sim::{Action, Course, Day, Hole, HoleStart, Player, Prompt, Skill, StrokeChoice};

let course = Course::open(Path::new("testdata/courses/test-course"))?;      // checks the manifest's SHA-256
let player = Player::default().with_skill(Skill::AMATEUR);        // the hands that press the meter
let mut hole = Hole::start(course.clone(), HoleStart::new(1, Day::Constant, 7, player))?;

while !hole.prompt().is_done() {
	let seen = hole.observe();                                    // only what the information mode shows a person
	let action = match hole.prompt() {
		Prompt::Play(_) => {
			let mut choice = StrokeChoice::stock(seen.advice.club, seen.aim_point);
			let preview = hole.preview(&choice)?;                 // the caddie's numbers, in Caddie mode
			choice.address.swing_length = fit(preview.carry_m, seen.to_cup_m * 0.94);
			Action::Stroke(choice)
		}
		Prompt::MaybeLost(p) if p.provisional_allowed => Action::Provisional,
		Prompt::MaybeLost(_) => Action::GoForward,
		Prompt::Choose(p) => Action::Choose(p.options[0]),
		Prompt::Drop(p) => Action::Drop { at: p.offered[0] },
		Prompt::Place(_) => Action::Place,
		Prompt::Done(_) => unreachable!(),
	};
	let step = hole.act(action)?;                                 // a refusal changes nothing
	println!("{:?}", step.stroke);
}
std::fs::write("hole.rec", hole.record().to_bytes())?;
```

`Round` has the same calls and adds `Action::PickUp` and the walk between holes.

### An agent over JSON

`golf play` reads one JSON request per line on stdin and writes one reply per line on stdout:

```text
$ golf play --course testdata/courses/test-course --hole 1 --day constant --seed 7 --skill amateur --mode caddie
> {"id":1,"op":"observe"}
< {"id":1,"ok":{"protocol":1,"prompt":{"play":{}},"hole":{"number":1,"par":4},"ball":{"spot":[12.5,301.2,-8.1],"lie":"tee"},
   "to_cup_m":351.7,"strokes":0,"wind":{"felt_mps":3.1,"from_deg":245.0},"address_elapsed_s":0.0,"advice":{"club":"driver"}}}
> {"id":2,"op":"preview","stroke":{"club":"driver","aim_at":[402.0,-88.5]}}
< {"id":2,"ok":{"carry_m":228.4,"plays_like_m":236.0,"wind_aloft":"more","roll_m":19.2}}
> {"id":3,"op":"act","action":{"wait":{"seconds":4.0}}}
> {"id":4,"op":"act","action":{"stroke":{"club":"driver","aim_at":[402.0,-88.5]}}}
< {"id":4,"ok":{"stroke":{"end":"at_rest","carry_m":224.9,"total_m":246.3,"lie":"fairway"},"ruling":null,"prompt":{"play":{}}}}
> {"id":5,"op":"act","action":{"drop":{"at":[1.0,2.0,3.0]}}}
< {"id":5,"refused":{"not_now":{"prompt":"play"}}}
> {"id":6,"op":"save","path":"runs/hole.rec"}
```

The operations are `observe`, `legal`, `preview`, `act`, `record` and `save`. Every double crosses the protocol exactly, because `serde_json` with its `float_roundtrip` feature writes the shortest text that reads back to the same bits. A test sent a million random doubles through it and got every one back unchanged. `golf schema` prints the protocol's JSON schema, which is a committed snapshot. Any change to it raises the `protocol` number.

Other commands:

- `golf batch --course <dir> --seeds 1-1000 --policy caddie --jobs 8` plays rounds in parallel and writes one JSON line per round, in seed order.
- `golf replay <record>` applies a record and names the first entry that diverges.
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

`GolfCourse` answers the queries a view makes to place or pick things (`ground_at`, `height_at`, `material_at`, `ray_hit`). It also hands over the payload's drawing data untouched. The binding names no class of any game. It converts the core's own values to Godot values and back, and every call runs on the main thread.

## The shape of the core

### One function applies every decision

A decision is the unit of play: a stroke, a drop, a choice of relief, a provisional ball, going forward, picking up. Each decision becomes a record entry, and one function applies every entry:

```text
person:  stamped key presses -> Console -> Action --\
agent:   Action -------------------------------------> resolve() -> Entry -> apply() -> Step
replay:  Entry with stored results -------------------------------^   compares, names the first divergence
```

- `Hole::act(action)` resolves an action into an entry: the address from its settings, the stance's footprint, the execution the hands produce, and the gust time. Then it calls `apply`.
- `apply(entry)` plays the stroke or the ruling, moves the clock and the wear, settles the lie, and asks the rules what comes next. With no stored results it stores them. With stored results it compares them exactly and refuses at the first difference.
- The prompt is derived from the rules state on every call and is never stored beside it.
- `observe()`, `legal()`, `preview()` and `prompt()` take `&self` and change nothing. `Hole` holds no interior mutability and no lazy cache, so it is `Send + Sync`, and an agent's search can run previews on many threads.

[ADR 0001](../adr/0001-every-decision-goes-through-one-apply.md) records this decision.

### The swing is pressed, not submitted

A person's swing goes through a meter: a press to start, a press at the power, a press when the ring closes, and the steering of the strike point. The `swing` crate judges those stamped presses into an `Execution`. The judgment is the same for a person and an agent:

- A person's presses come from the engine, stamped at the time they happen.
- An agent submits an intent: the club, the address, the aim and the strike point. A `Skill` performs it. Its hands press a real meter with seeded errors in a person's units, milliseconds and millimetres, drawn from `Seed::derive(attempt_seed, "hands")`.

A hard lie narrows the timing window, so the same skill faults more there, as a person does. `Skill::PERFECT` presses on the mark and is for planning and tests. No public type lets a caller hand the core an `Execution`. Only the meter and replay build one, so an agent's score means what a person's score means.

`Action::Wait { seconds }` lets address time pass, as a person watching the flags does. The gust a stroke meets depends on that time. A wait is not a record entry, because the stroke entry stores its gust time.

### The person's console

A person drives a game with key presses, not decisions. The `Console` turns stamped input into actions. It owns the state that belongs to a screen: the address being built, the swing meter in progress, the relief option under the cursor, the drop spot being picked, the club the caddie handed over, the briefing, and the end of a flight's playback. It schedules the caddie's preview 150 ms after a change settles. Its `view(now)` builds the values a screen draws and changes nothing.

The console holds no rule. It only produces actions, so a person and an agent reach the same `apply`.

### The world one stroke reads

A `World` is a value that holds the ground, the air, the cup, the clock, the legal areas and the round's moment. A stroke reads one world. The world changes only when `apply` commits a stroke, through a new clock or new wear.

- `Ground` and `Air` are closed enums with a native form for every case: the course's ground, a generated day's ground, the practice ground, a plane, and the stroke's own wrappers. The physics never calls out of the core.
- A generated day is a `RoundPlan` that holds every hourly state. No layer is computed during play.

### Crates

The crates follow the dependency order of the domain, so the compiler enforces which layer may use which.

| Crate | Owns | Depends on |
| --- | --- | --- |
| `golf-sim-base` | `fm` (every transcendental), engine-order vectors, plane geometry, seeds, the Variant codec, the bundle container, digests | nothing |
| `golf-sim-physics` | Flight, bounce, roll, the cup, obstacles, the strike, tunables, the sampler traits, the plane ground, constant air, the zone map | base |
| `golf-sim-course` | The course bundle reader, grids, materials, regions, obstacles, the legal map, `ray_hit` | base, physics |
| `golf-sim-conditions` | The world, the samplers, wear, lies, the round plan, the generator, course analysis | base, physics, course |
| `golf-sim-swing` | The meter judgment and the skilled hands | base |
| `golf-sim-stroke` | The bag, the address, the delivery, a stroke, a preview, a drop | base, physics, course, conditions, swing |
| `golf-sim-rules` | The hole's rules state, rulings, relief | base, physics, course, stroke |
| `golf-sim` | `Hole`, `Round`, `Action`, records, observation, the console, the range | every crate above |
| `golf-sim-cli` | The `golf` program | golf-sim |
| `golf-sim-godot` | The Godot binding, a `cdylib` | golf-sim |
| `golf-sim-testkit` | The determinism suite and the fixture loaders, for tests only | golf-sim |

`golf-sim`, `golf-sim-cli` and `golf-sim-godot` are the entry points. The other crates are internal and are not published on their own. A consumer depends on `golf-sim`, and a check in the consuming game fails if it names an internal crate.

## The determinism rules

These rules make a test run on one machine stand for every platform. Each rule has a check that fails the build when it is broken.

| Rule | Check |
| --- | --- |
| One pure-Rust maths library computes every transcendental: the `libm` crate with its `force-soft-floats` feature, called only through `golf_sim_base::fm`. The core never calls the platform's maths library. | Clippy's `disallowed-methods` bans every `f32` and `f64` transcendental method (`sin`, `cos`, `tan`, `atan2`, `exp`, `ln`, `log10`, `powf`, `powi`, `sqrt`, `hypot`, `sin_cos` and the rest) in every crate. `fm` calls the `libm` crate's functions, not std's. |
| No fused multiply-add, written or produced by a compiler flag. | `mul_add` is a disallowed method. The workspace sets no `target-cpu` or `target-feature`. A test computes `a * b + c` for inputs where a fused result differs and requires the unfused one. |
| No hash-map order reaches an outcome. Iterate ordered maps, or sort before iterating. | `disallowed-types` bans `std::collections::HashMap` and `HashSet` in every crate. |
| Parallel work reduces in a fixed order. The core starts no thread. | `disallowed-methods` bans `std::thread::spawn` and `std::thread::scope` outside `golf-sim-cli` and `golf-sim-godot`. A test fails when a core crate's dependency tree holds a thread pool. `golf batch` runs independent rounds in parallel and writes them in seed order. The binding may move a job to its own thread, but the job is a whole stroke or preview, so no result depends on which thread ran it. |
| The core computes in 64-bit floats. Single precision appears only where a format stores it, and the codec widens it on read. | A test scans the core crates for `f32` outside the codecs and the [reference configuration](#proof-against-the-reference). |
| Loading a course does not depend on path separators or line endings. | `Course::open` joins paths with `Path::join`, resolves manifest names by their `/` components, parses JSON, and hashes bytes, never text. `.gitattributes` keeps text at LF and marks fixtures binary. A test opens a bundle through a path with `.` and `..` parts and a manifest rewritten with CRLF, and requires the same revision and the same course. |

[ADR 0002](../adr/0002-one-pure-rust-maths-library-on-every-platform.md) records the maths decision. The pure maths library and Apple's library disagree by one unit in the last place on 1% to 18% of inputs, depending on the function. So a result computed with the platform's library would differ between machines, and these rules exist to stop that.

### When the platforms are checked

- **Every change, on the developer's Mac.** `cargo test --workspace` runs the unit tests and the determinism suite. The rules above remove the known causes of a platform difference, so a pass on one machine is strong evidence for the others.
- **Each release, and on demand.** The [release workflow](#build-and-release) runs the same tests and the determinism suite on macOS, Linux and Windows. A platform difference shows up there and nowhere earlier.
- **Before a release.** Run the release workflow by hand (`workflow_dispatch`). It is the tool for checking the three platforms before a tag. The build plan runs it at the end of every phase, so a difference is found in the phase that caused it.

### The determinism suite

The suite plays a fixed set of strokes and seeded rounds on the synthetic test course and computes the SHA-256 of each result's Variant bytes. Each layer keeps its items in `crates/golf-sim-testkit/src/items/<layer>.rs` and their expected digests in `testdata/determinism/<layer>.json`. A change that moves a digest fails the suite until the change raises the version it affects and the layer is stored again with `cargo run -p golf-sim-testkit -- bless <layer>`.

## Proof against the reference

Each porting unit proves its layer against data the GDScript reference produced. The reference computes vectors in single precision and takes its transcendentals from the platform's maths library. The core does neither, so its default build cannot match the reference bit for bit.

A **reference configuration** exists for the comparison alone. The `reference` feature makes the engine-order vectors single precision and lets `fm` call std's maths functions, on macOS arm64, the platform the reference data comes from. It is not part of any release. The release workflow fails when the feature is on, and the feature is deleted when the port is complete. In that configuration a porting unit must match the reference bit for bit. A difference is a bug in the port, so the comparison catches the kind of slip that changes a putt's break and raises no error, such as reading a value before or after it is resampled.

The default configuration then differs from the reference by the two changes alone, 64-bit vectors and the pure maths library. The versions record the difference. The default build writes a `config` key into every version table and its own physics, generator and analysis versions, so a record or a cached analysis made by one configuration is refused by the other.

Once a unit matches the reference, `golf corpus twin` stores its **twins**: the same inputs with the default build's outputs. The default build's tests compare against the twins, so every layer keeps a test after the reference configuration is gone. `golf corpus drift` compares a fixture with its twins. It reports, for each numeric field, the largest absolute and relative difference, and it lists every case whose discrete result differs, such as an end reason, a surface, a prompt or a holed ball.

Reference data comes in three tiers:

- **Layer fixtures.** One call's inputs and outputs, such as a launch, a ground sample, a ruling, or a pass of the generator.
- **Records.** A hole or a round played by the reference. The core must replay every record to the same bytes. The reverse also holds: the reference must replay a record the core wrote.
- **Traces.** Every stamped command a test sends to the reference's session, with each event and view it returned. The console must return the same.

Fixture files are Variant bytes with a short header: the magic `GSCORP01`, a u32 length, and a dictionary `{format, layer, oracle: {versions, commit}, cases: [{id, call, input, output}]}`. Variant bytes are the one format the reference writes with exact doubles. Fixtures on the synthetic test course and on flat ground live in `testdata/<layer>/<set>/`, and the first unit that compares against a set commits it. `golf corpus check` reports the path of the first value that differs, such as `cases[12].output.events[3].time_s`. A trace compares a projection of each view: no text, the course by its revision, and a stroke's result by its digest.

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

A hole record is the magic `LLHOLE01`, a u32 length, and a Variant dictionary: `{format, versions, course_revision, setup, settings, entries}`. A round record is `LLROUND1`, a u32 length, and `{format, versions, course_id, course_name, course_revision, request, generator, tees, order, pick_up_rule, holes}`, where each hole holds its own record's bytes. The reference configuration writes hole format 4 and round format 2, as the reference does. The default configuration stores every position as 64-bit floats and writes hole format 5 and round format 3.

### Digests and fingerprints

A digest is the SHA-256 of a value's Variant bytes, where an object becomes a dictionary of its fields in declaration order. A round plan's fingerprint is the digest of its layers. Its digest adds the request, the generator version, the course revision, and the analysis and physics versions. A request that carries a fingerprint generates that round or an error.

## Build and release

### Building

The workspace needs Rust 1.94 or later. `cargo build` and `cargo test` work offline once the crates are fetched. To build where the network is closed, run `cargo vendor <dir>` with the network once and point Cargo at the folder with the `[source]` lines that `cargo vendor` prints. Put those lines in an untracked `.cargo/config.toml`, never in the repository.

### The release workflow

`.github/workflows/release.yml` builds and checks the core on three platforms. It runs only when a tag matching `v*` is pushed, when a GitHub release is published, or when someone starts it by hand. No workflow runs on a push or a pull request.

| Platform | Runner | Target |
| --- | --- | --- |
| macOS | `macos-14` | `aarch64-apple-darwin` |
| Linux | `ubuntu-latest` | `x86_64-unknown-linux-gnu`, built with `cargo zigbuild` against glibc 2.28 |
| Windows | `windows-latest` | `x86_64-pc-windows-msvc` |

On each runner the workflow runs `cargo test --workspace` with the determinism suite, then builds the release artifacts: the `golf` program and the `golf-sim-godot` library. A final job compares the three platforms' determinism digests and fails on any difference. A tagged run attaches the artifacts and a `SHA256SUMS` file to the GitHub release. The workflow uses hosted runners only.

The glibc floor costs nothing in determinism, because the core takes no maths from glibc. A macOS x86_64 or universal library is left for the first macOS export that needs one. It would be one more target on the same runner.

## Units

The core is built in small units. Each unit lands on its own on `main` with its tests, and each has a check that proves it done. Units in the same track touch different files, so they run at the same time. A unit marked **codex** is fully specified, checked by tests or a byte-for-byte comparison, and runs headless and offline. A unit marked **claude** needs the network or GitHub, a decision that is still open, a real window, or changes across repositories.

Every unit follows the determinism rules and builds offline. A porting unit's issue names the files, the fixtures it compares against, and the commands that prove it. A unit that needs data from the reference waits for it, and the table names that wait as reference data from the consuming game.

<!-- units:begin -->
Sizes are estimates in lines of Rust, tests included. Some units also wait for reference data that the consuming game produces. Units whose dependencies are met run at the same time.

### Phase 0. Foundation

| Unit | Title | Track | Label | Depends on | Size |
| --- | --- | --- | --- | --- | ---: |
| [C01](https://github.com/scudellari/golf-sim/issues/1) | Workspace skeleton, shared types, determinism lints and offline builds | Shared base | claude | nothing | ~500 |
| [C02](https://github.com/scudellari/golf-sim/issues/2) | Maths: `fm`, engine-order vectors and the engine's built-ins | Maths and determinism | codex | C01, reference data from the consuming game | ~550 |
| [C03](https://github.com/scudellari/golf-sim/issues/3) | Plane geometry: point in polygon, nearest point, segment crossing, cell-run union | Maths and determinism | codex | C01, reference data from the consuming game, a step in the consuming game | ~300 |
| [C04](https://github.com/scudellari/golf-sim/issues/4) | Seeds: `Seed::derive` and `SeedStream` | Maths and determinism | codex | C01, reference data from the consuming game | ~180 |
| [C05](https://github.com/scudellari/golf-sim/issues/5) | Variant codec, ordered dictionary, bundle container and digests | Variant, bundle and records | codex | C01, reference data from the consuming game | ~550 |
| [C06](https://github.com/scudellari/golf-sim/issues/6) | The determinism suite | Shared base | codex | C02, C04, C05 | ~250 |
| [C48](https://github.com/scudellari/golf-sim/issues/7) | Twin fixtures, the drift report and `golf corpus` | Shared base | codex | C05, C06 | ~400 |
| [C07](https://github.com/scudellari/golf-sim/issues/8) | Release workflow: tests, determinism and artifacts on macOS, Linux and Windows | Release | claude | C06, C48 | ~200 |

### Phase 1. Strokes on a constant day

| Unit | Title | Track | Label | Depends on | Size |
| --- | --- | --- | --- | --- | ---: |
| [C08](https://github.com/scudellari/golf-sim/issues/9) | Course data: typed parse and grids | Course | codex | C05, reference data from the consuming game | ~550 |
| [C10](https://github.com/scudellari/golf-sim/issues/10) | Physics values, sampler traits, the plane ground and constant air | Physics solver | codex | C02, C03, reference data from the consuming game | ~550 |
| [C09](https://github.com/scudellari/golf-sim/issues/11) | Course obstacles, zone map, legal map and the stance footprint | Course | codex | C08, C03, C10 | ~550 |
| [C11](https://github.com/scudellari/golf-sim/issues/12) | Flight: atmosphere, aerodynamics and the flight step | Physics solver | codex | C10 | ~300 |
| [C12](https://github.com/scudellari/golf-sim/issues/13) | Ground contact: bounce, roll, the cup and solid sweep | Physics solver | codex | C10 | ~400 |
| [C13](https://github.com/scudellari/golf-sim/issues/14) | The stroke integrator | Physics solver | codex | C11, C12 | ~800 |
| [C14](https://github.com/scudellari/golf-sim/issues/15) | Club impact and strike contact | Physics solver | codex | C10 | ~550 |
| [C15](https://github.com/scudellari/golf-sim/issues/16) | The physics facade | Physics solver | codex | C13, C14 | ~300 |
| [C16](https://github.com/scudellari/golf-sim/issues/17) | The world on a constant day | World and stroke | codex | C09, C15, reference data from the consuming game | ~550 |
| [C17](https://github.com/scudellari/golf-sim/issues/18) | Stroke setup: address, bag, tuning and delivery | World and stroke | codex | C16 | ~600 |
| [C18](https://github.com/scudellari/golf-sim/issues/19) | The stroke facade, `golf stroke` and `golf course check` | World and stroke | codex | C17 | ~500 |

### Phase 2. A hole on a constant day

| Unit | Title | Track | Label | Depends on | Size |
| --- | --- | --- | --- | --- | ---: |
| [C19](https://github.com/scudellari/golf-sim/issues/20) | The swing meter's judgment | Swing and rules | codex | C02, C04, reference data from the consuming game | ~350 |
| [C20](https://github.com/scudellari/golf-sim/issues/21) | Skilled hands: an intent pressed on the real meter | Swing and rules | codex | C19 | ~250 |
| [C21](https://github.com/scudellari/golf-sim/issues/22) | Rules: rulings, prompts and the provisional ball | Swing and rules | codex | C18, C09, reference data from the consuming game | ~450 |
| [C22](https://github.com/scudellari/golf-sim/issues/23) | Relief: areas, complete relief, abnormal conditions, drops and placement | Swing and rules | codex | C21, C03 | ~450 |
| [C23](https://github.com/scudellari/golf-sim/issues/24) | Hole records and the hole's setup | Variant, bundle and records | codex | C05, C17, C19, C16, reference data from the consuming game | ~450 |
| [C51](https://github.com/scudellari/golf-sim/issues/25) | The hole's relief ground | Hole, agent and CLI | codex | C22, C23 | ~250 |
| [C24](https://github.com/scudellari/golf-sim/issues/26) | The hole: actions, entries, `apply` and replay | Hole, agent and CLI | codex | C18, C20, C51 | ~450 |
| [C25](https://github.com/scudellari/golf-sim/issues/27) | The hole: prompts, legal actions, previews and the caddie's advice | Hole, agent and CLI | codex | C24 | ~350 |
| [C26](https://github.com/scudellari/golf-sim/issues/28) | Observation: what each information mode shows | Hole, agent and CLI | codex | C25 | ~400 |
| [C27](https://github.com/scudellari/golf-sim/issues/29) | `golf play`, `golf replay` and `golf schema` | Hole, agent and CLI | codex | C26 | ~500 |
| [C28](https://github.com/scudellari/golf-sim/issues/30) | `golf batch` and the built-in caddie policy | Hole, agent and CLI | codex | C27 | ~350 |

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
| [C34](https://github.com/scudellari/golf-sim/issues/37) | The round: holes in order, the walk, the card and the round record | Round ground, air and round | codex | C33, C24 | ~550 |
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
| [C50](https://github.com/scudellari/golf-sim/issues/49) | `golf fit`: the calibration commands | Tools | codex | C15, reference data from the consuming game | ~300 |

### Phase 7. The game on the core

| Unit | Title | Track | Label | Depends on | Size |
| --- | --- | --- | --- | --- | ---: |
| [C45](https://github.com/scudellari/golf-sim/issues/50) | The Godot binding | Godot binding | codex | C49, C31, C42 | ~600 |

### Phase 8. Retire the GDScript simulation

| Unit | Title | Track | Label | Depends on | Size |
| --- | --- | --- | --- | --- | ---: |
| [C46](https://github.com/scudellari/golf-sim/issues/51) | Remove the reference configuration | Maths and determinism | codex | a step in the consuming game | ~200 |
<!-- units:end -->

## Questions still open

- Which skill counts as an agent playing the game, and whether `Skill::PERFECT` may play a scored round.
- Whether an agent may wait at address to time a gust.
- How much of the course an agent may read through the program. The library can read all of it.
