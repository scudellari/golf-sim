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

# Copy every crate the workspace uses into DIR for offline builds, and print the Cargo config that reads it.
vendor dir:
    cargo vendor --locked --versioned-dirs "{{absolute_path(dir)}}"
