# List the recipes.
default:
    @just --list

# Run the tests of every crate. Extra arguments go to cargo test.
test *args:
    cargo test --workspace {{args}}

# Check the formatting and every lint, the determinism lints included.
lint:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings

# Replay a bundle under LLDB. Extra arguments go to LLDB.
[positional-arguments]
debug-replay bundle *lldb_args:
    cargo build --offline -p golf-sim-cli --bin golf --profile debugging
    bundle=$1; shift; exec lldb "$@" -- target/debugging/golf replay "$bundle"

# Copy every crate the workspace uses into DIR for offline builds, and print the Cargo config that reads it.
vendor dir:
    cargo vendor --locked --versioned-dirs "{{absolute_path(dir)}}"
