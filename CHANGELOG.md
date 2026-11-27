# Changelog

## 0.2.1 (2026-11-27)

- **`golf corpus` checks every phase 1 layer.** `golf corpus check` and `golf corpus drift` run course, physics, world and stroke fixtures through the same runners as the tests, and `--course-dir` opens the course a fixture names, so a consumer can compare its own courses' fixtures.
- **Repro bundles and debugging.** Hole repro bundles with `golf replay`, `golf explain` and `golf scenario from-repro`, a `debugging` build profile, `just debug-replay <bundle>` and the debugging guide in `docs/how-to/debug.md`.
- **Stable fixtures.** Determinism digests and committed records no longer change with the release number.

## 0.2.0 (2026-11-27)

A stroke plays on a constant day, and a hole plays to the cup. The physics is version 0.11.0, checked against published measurements, and every layer was compared with the earlier implementation within its stated tolerance.

### What is in it

- **The physics.** Flight, ground contact, bounce, roll, the cup, the flagstick, club impact and the stroke integrator, with tunables fitted to measured data. Since the port, three model changes have landed, each scored before and after: the flagstick's rim response, cup capture on slopes, and green bounce across the iron range. A lift cap at high spin was tried and held, because no form passed the rule that a change may not make any target worse.
- **Validation.** `golf validate` prints the residuals against the measured targets, `golf score` compares two models, and `golf fit` drives the calibration script. The licence ledger lists every dataset.
- **The course and the world.** Course grids, obstacles, the legal map, the stance footprint, the constant-day world, lies and wear.
- **A stroke.** The address, the bag, the delivery, `golf stroke`, `golf course check`, stroke traces, `golf explain` and stroke repro bundles.
- **The swing.** The swing meter's judgment from stamped presses, exact swings, and the provisional `scratch` and `hcp15` skill profiles.
- **The rules and the hole.** Rulings, prompts, the provisional ball, relief, hole records (format 5), the caddie's advice and the HUD's numbers. `Hole` applies every decision through one function and replays its own records bit for bit.
- **The program.** `golf play` speaks a JSON-lines protocol (protocol 1, with `golf schema`), `golf replay` names the first entry that diverges, `golf batch` plays holes on parallel threads with the scripted golfer, `golf sweep` reports score distributions over profiles and seeds, and `golf scenario` checks scripted holes against expected results.

The downloads and their checksums are attached to the release, as for 0.1.0.

## 0.1.0 (2026-11-26)

This is a foundation release. It has no physics. The `golf` program cannot play a stroke, a hole or a round, and most of its commands report that they are not built yet. The Godot library builds but registers no classes.

The release publishes the base that the physics will stand on, and it proves that one build gives the same results on macOS, Linux and Windows.

### What is in it

- **The workspace.** The crates, the shared types, the clippy lints that reject platform maths, and a test that rejects `f32` outside the codecs. The crates build offline from a vendored copy, as `CONTRIBUTING.md` describes.
- **Maths.** Every transcendental goes through `fm`, which uses `libm` on the soft-float path on every platform (ADR 0002). The release also has Godot's vector operations, computed in Godot's order, and Godot's scalar maths built-ins.
- **Plane geometry.** Point in polygon, nearest point, segment crossing, and the union of cell runs.
- **Seeds.** Seed derivation and deterministic random streams.
- **The course-file reader.** Godot's zstd block container, the course manifest, the Variant codec, and course bundle checks. A synthetic test course is in `testdata/courses/test-course/`.
- **The determinism suite.** A layered suite that stores a digest for each result. The release workflow runs it on all three platforms and fails when any digest differs. `golf corpus check` and `golf corpus drift` compare a recorded fixture against the tolerance table.
- **Logging.** `golf --log FILE` writes JSON log events, and `--log-level` sets the most verbose level.
- **Traces.** The stroke phases, probes and simulation spans that later releases record for each stroke.
- **The release workflow.** A pushed `v*` tag builds, tests and compares digests on the three platforms, then attaches the files below to the GitHub release.

### Downloads

| File | Platform |
| --- | --- |
| `golf-macos`, `libgolf_sim_godot-macos.dylib` | macOS, Apple silicon (`aarch64-apple-darwin`) |
| `golf-linux`, `libgolf_sim_godot-linux.so` | Linux x86_64, glibc 2.28 or later |
| `golf-windows.exe`, `golf_sim_godot-windows.dll` | Windows x86_64 (MSVC) |
| `SHA256SUMS` | The SHA-256 checksum of each file |

The macOS files are not signed or notarized.
