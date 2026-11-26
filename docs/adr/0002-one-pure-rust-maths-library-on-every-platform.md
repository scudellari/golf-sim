# 0002. One pure-Rust maths library on every platform

- Status: Accepted
- Date: 2026-11-26

## Context

The same seed must play the same round on macOS, Linux and Windows, so saved rounds and replays work across machines. IEEE addition, multiplication, division and square root give the same bits everywhere, provided no multiply-add is fused. Transcendentals come from each platform's maths library, and those differ in the last bit. A measurement on Apple's library against the pure-Rust `libm` crate 0.2.16, a million inputs per function, found these shares of results that differ by one unit in the last place: `sin` 4.3%, `cos` 4.5%, `exp` 9.8%, `log` 1.1%, `pow` 9.7%, `atan2` 18.0%, `sqrt` 0%.

## Decision

- The core computes every transcendental with the `libm` crate, built with `force-soft-floats` so no platform-specific path runs, and called only through `golf_sim_base::fm`.
- The core never fuses a multiply-add, starts no thread, lets no hash-map order reach an outcome, computes in 64-bit floats, and loads a course the same way whatever the path separators and line endings.
- Clippy lints and tests enforce each rule, and the determinism suite runs on all three platforms in the release workflow.
- While the port lasts, a `reference` feature mirrors the GDScript reference (single-precision vectors and the platform's maths) for comparisons on macOS only. No release builds it, and it is deleted when the port is complete.

## Alternatives considered

- **The platform's maths library.** Each platform would play a different round.
- **A correctly rounded maths library.** It would also be identical everywhere, at a higher cost per call, and the `libm` crate is already portable once its soft-float path is forced.
- **Match the reference first and switch later.** The shipped core would carry the platform's maths for months, and every saved round of that period would be refused at the switch.

## Consequences

- A test pass on one machine stands for the other platforms, and the release workflow confirms it.
- The default build differs from the reference by these two changes alone. It reports its own physics, generator and analysis versions, and a drift report measures the difference on the reference's data.
- A platform difference that slips past the rules shows up only in the release workflow, which runs by hand before every release.
