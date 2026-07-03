# List available recipes
default:
    @just --list

# Format all code
fmt:
    cargo fmt

# Check formatting without modifying
fmt-check:
    cargo fmt --check

# Lint with clippy across all targets and features; warnings are errors
clippy:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

# Run the full test suite (all features)
test:
    cargo test --workspace --all-features

# Quick tests with default features only
test-fast:
    cargo test --workspace

# Build docs like docs.rs does; warnings are errors
doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps

# Everything CI checks, in one command
ci: fmt-check clippy test doc

# Install the `snowdrop` CLI from the working tree
install-cli:
    cargo install --path snowdrop-id-cli

# Verify both crates package and build cleanly (workspace-aware: the CLI
# is verified against the freshly packaged lib, not crates.io)
package:
    cargo package --workspace

# Publish both crates; cargo orders them (lib first) and waits for indexing
publish: ci
    cargo publish --workspace
