# The shared workspace version, e.g. "0.3.0"
version := `grep -m1 '^version' Cargo.toml | cut -d '"' -f 2`

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

# Run the full test suite (all features); Postgres tests skip without SNOWDROP_TEST_PG_URL
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

# Verify all crates package and build cleanly (dependents vs. freshly packaged libs)
package:
    cargo package --workspace

# Bump version + deps + CHANGELOG and commit "Release vX.Y.Z" on a feature branch (see docs/RELEASING.md)
bump level:
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{level}}" in
        patch|minor|major) ;;
        *) echo "usage: just bump <patch|minor|major>" >&2; exit 1 ;;
    esac
    branch=$(git rev-parse --abbrev-ref HEAD)
    if [[ "$branch" == "main" ]]; then
        echo "refusing to bump on main; create a feature branch first" >&2
        exit 1
    fi
    if [[ -n "$(git status --porcelain)" ]]; then
        echo "working tree is dirty; commit or stash first" >&2
        exit 1
    fi
    cur="{{version}}"
    IFS=. read -r major minor patch <<< "$cur"
    case "{{level}}" in
        patch) patch=$((patch + 1)) ;;
        minor) minor=$((minor + 1)); patch=0 ;;
        major) major=$((major + 1)); minor=0; patch=0 ;;
    esac
    new="${major}.${minor}.${patch}"
    echo "Bumping ${cur} -> ${new}"
    # Workspace version.
    sed -i -E "s/^version = \"[^\"]*\"/version = \"${new}\"/" Cargo.toml
    # Internal dependency requirements (published crates stay in lockstep).
    sed -i -E "s/(snowdrop-id = \{ version = )\"[^\"]*\"/\1\"${new}\"/" \
        snowdrop-id-cli/Cargo.toml snowdrop-id-postgres/Cargo.toml
    # Refresh the workspace crate versions in Cargo.lock.
    cargo update --workspace --quiet
    # Stamp the CHANGELOG's Unreleased section with the version and date.
    if grep -q '^## \[Unreleased\]' CHANGELOG.md; then
        sed -i -E "s/^## \[Unreleased\]/## [${new}] - $(date +%F)/" CHANGELOG.md
    else
        echo "warning: no '## [Unreleased]' section in CHANGELOG.md to stamp" >&2
    fi
    git add Cargo.toml Cargo.lock CHANGELOG.md \
        snowdrop-id-cli/Cargo.toml snowdrop-id-postgres/Cargo.toml
    git commit -m "Release v${new}"
    echo "Committed 'Release v${new}'. Push this branch, open a PR, and after it"
    echo "merges to main run 'just publish'."

# Tag v<version> on main and push it to trigger the crates.io publish workflow (see docs/RELEASING.md)
publish:
    #!/usr/bin/env bash
    set -euo pipefail
    branch=$(git rev-parse --abbrev-ref HEAD)
    if [[ "$branch" != "main" ]]; then
        echo "publish must be run from main (currently on '$branch')" >&2
        exit 1
    fi
    if [[ -n "$(git status --porcelain)" ]]; then
        echo "working tree is dirty; commit or stash before releasing" >&2
        exit 1
    fi
    git pull --ff-only
    tag="v{{version}}"
    if git rev-parse "$tag" >/dev/null 2>&1; then
        echo "tag $tag already exists; did you forget to 'just bump'?" >&2
        exit 1
    fi
    git tag -a "$tag" -m "snowdrop-id workspace $tag"
    git push origin "$tag"
    echo "Pushed $tag. The publish workflow will build and publish the crates."
