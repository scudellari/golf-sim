# Contributing to golf-sim

golf-sim must play the same round from the same seed on macOS, Linux and Windows. Most of these rules exist for that. [`docs/design/core.md`](docs/design/core.md) explains the design, and [`docs/adr/`](docs/adr/README.md) records the decisions behind it.

## Build and test

You need Rust 1.94 or later and [`just`](https://github.com/casey/just).

| Task | Command |
| --- | --- |
| Run every test | `just test` |
| Check the formatting and the lints | `just lint` |
| Vendor the crates for an offline build | `just vendor <dir>` |

Before you push, run `just test` and `just lint`. Both must pass.

### Build offline

Fetch the crates once with the network:

```sh
just vendor /path/to/vendor
```

The recipe prints a `[source]` table. Put it in `.cargo/config.toml` at the root of your checkout. That file is ignored by git, so never commit it. Then build with `--offline`:

```sh
cargo build --workspace --offline
cargo test --workspace --offline
```

When `Cargo.lock` changes, run `just vendor` again.

### Validation data and fixtures

Extra validation data and reference cases can be supplied through `GOLF_SIM_LOCAL_DATA` or `testdata/local/`, each with its own ledger (`validation/targets/LICENCES.toml` and `fixtures/LICENCES.toml`), and are skipped when absent.

Set `GOLF_SIM_LOCAL_DATA=` to run in public mode. For validation output from passing tests, use `cargo test --workspace --offline -- --nocapture`. See the [calibration guide](calibration/README.md).

## The determinism rules

| Rule | What to do | Check |
| --- | --- | --- |
| One maths library | Call every transcendental (`sin`, `atan2`, `exp`, `powf`, `sqrt` and the rest) through `golf_sim_base::fm`, which uses the `libm` crate. Never call the `f32` or `f64` method. | `disallowed-methods` in `clippy.toml` |
| No fused multiply-add | Write `a * b + c`. Never call `mul_add`, and never set `target-cpu` or `target-feature`. | `disallowed-methods` in `clippy.toml` |
| No hash order | Use `BTreeMap`, `BTreeSet` or a sorted `Vec`. | `disallowed-types` bans `HashMap` and `HashSet` |
| No threads in the core | Only `golf-sim-cli` and `golf-sim-godot` may start a thread. Parallel work reduces in a fixed order. | `disallowed-methods` bans `std::thread::spawn` and `std::thread::scope`, and the `dependencies` test fails when a core crate depends on a thread pool |
| 64-bit floats | Compute in `Real` (`f64`). Use `f32` only where a stored format has it, and widen it when you read it. | The `no_f32` test in `golf-sim-testkit` scans core source files outside the codecs |
| Paths and line endings do not matter | Join paths with `Path::join`, and hash bytes, never text. | `.gitattributes` keeps text at LF and fixtures binary |

Clippy reads the nearest `clippy.toml` above a crate. `crates/golf-sim-cli` and `crates/golf-sim-godot` keep a copy of the workspace's file without the thread entries. If you change `clippy.toml`, change both copies. The `lint_config` test in `golf-sim-testkit` fails until they match, and it runs clippy on `crates/golf-sim-testkit/lint-fixture` to prove that each banned call is caught.

Compare the core's results with the reference implementation's recorded data within the stated tolerance for each layer.

## Where code goes

Each crate owns one layer of the domain, and a crate may use only the crates below it. The table in [Crates](docs/design/core.md#crates) lists them. Every module the plan names already exists as a stub, so fill the files your change names and leave each `lib.rs`, `main.rs` and `Cargo.toml` alone. Each `golf` subcommand lives in its own file under `crates/golf-sim-cli/src/cmd/`, which defines that file's `Command` enum and its `run` function.

Keep hot paths free of macros that hide control flow from a debugger. See the [debugging guide](docs/how-to/debug.md) for replay and breakpoint steps.

## Commits

- Use a conventional prefix: `feat:`, `fix:`, `docs:`, `refactor:`, `test:` or `chore:`.
- Land each change with its tests.
- Write an ADR when a decision changes a crate boundary, a public type, a file format, a determinism rule or the release process. [`docs/adr/README.md`](docs/adr/README.md) has the template.
