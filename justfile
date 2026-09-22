# List the available recipes.
default:
    @just --list

# Check formatting without modifying files.
fmt:
    cargo fmt --check

# Apply rustfmt to the sources.
fmt-fix:
    cargo fmt

# Lint all targets with Clippy, warnings as errors.
clippy:
    cargo clippy --all-targets -- -D warnings

# Run the test suite.
test:
    cargo test

# Run every quality gate: fmt, clippy, test.
check: fmt clippy test

# Build the debug binary.
build:
    cargo build

# Build the release binary.
build-release:
    cargo build --release

# Run the TUI, optionally against a vault: just run /tmp/vault
run vault="":
    cargo run -- {{ if vault == "" { "" } else { "--vault " + quote(vault) } }}

install: build-release
    rm ~/.local/bin/tt | true
    ln -sf "$(pwd)/target/release/tt" ~/.local/bin/tt

# Run the throwaway footer-design prototypes.
proto:
    cargo run --manifest-path prototypes/footer/Cargo.toml
