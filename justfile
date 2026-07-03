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

# Verify both crates package cleanly (CLI skips build verification:
# its snowdrop-id dependency isn't on crates.io until publish)
package:
    cargo publish -p snowdrop-id --dry-run
    cargo package -p snowdrop-id-cli --no-verify

# Publish both crates: the lib first, then the CLI once it's indexed
publish: ci
    cargo publish -p snowdrop-id
    @echo "Waiting for crates.io to index snowdrop-id ..."
    sleep 60
    cargo publish -p snowdrop-id-cli
