# 0006. Every result can be replayed and explained

- Status: Accepted
- Date: 2026-11-26

## Context

The owner asked on 2026-11-26 that the core make it easy to understand what happened: "It should be fully debuggable by a programmer or AI agent. We should be able to set breakpoints, etc. We should be able to set seeds to replay something that failed previously. We should have appropriate logging to have visibility into behavior. When playing the game I should be able to give feedback that something felt wrong and the AI agent should have all the information it needs to troubleshoot and get to the bottom of it."

## Decision

- **Records reproduce.** Every stroke repro, hole record and round record carries its seeds, the release and commit, every model version, the course revision, the conditions request, and each decision with the swing that made it. `golf replay` reproduces it bit for bit.
- **Repro bundles.** A folder with a manifest, the record's bytes, and optionally traces and a log tail. A failing test saves one, a game's feedback key writes one with more files, and one command turns one into a regression scenario.
- **Stroke traces.** Physics functions take a `Probe`. `NoProbe` costs nothing, and `Trace` records each phase's inputs, key values and outputs, each named by the tunable or data it came from. `golf explain` prints the account and the JSON.
- **Logging.** The `tracing` crate, with spans for the round, the hole, the stroke and each phase, and JSON output to a file.
- **Watching never changes a result.** Tests require the same bits with a trace on and off, and the same determinism digests with logging on and off.
- **Breakpoints.** Debug builds keep symbols, a `debugging` profile gives an optimized build with full debug information, and a guide covers the CLI and an attached Godot.

## Alternatives considered

- **Logging alone.** A log shows what happened in the order it happened, but it cannot name the tunable behind a number unless every log line does, and it costs something in every stroke.
- **Traces as `tracing` events.** One mechanism instead of two, but the values would be text in a log, not a typed record that `golf explain` can narrate and a test can compare.

## Consequences

- A bug report becomes a bundle, a bundle becomes a replay under a debugger, and a fixed bug becomes a scenario.
- Physics functions carry one more generic parameter.
- The trace and replay units land in phases 1 and 2, beside the physics they explain.
