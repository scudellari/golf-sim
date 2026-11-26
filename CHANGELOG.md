# Changelog

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
