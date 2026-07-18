# Releasing

The three crates (`snowdrop-id`, `snowdrop-id-cli`, `snowdrop-id-postgres`) are
versioned in lockstep and published to crates.io from CI using **Trusted
Publishing (OIDC)** — GitHub Actions mints a short-lived crates.io token per run,
so there is no long-lived `CARGO_REGISTRY_TOKEN` secret to leak or rotate.

**Why lockstep (for now):** it keeps one version, one CHANGELOG, and one tag,
which is worth more than the noise it costs while the project is pre-1.0 and has
few users. The cost is that a breaking change to `snowdrop-id-postgres` (the
crate that churns) drags the spec-frozen `snowdrop-id` core through a version
bump it didn't earn. Revisit **independent versioning around 1.0**, when the
core's stability becomes a real promise and the leasing crate is still moving.

## Release flow

1. **On a feature branch, bump the version** (the commit rides along in a normal
   PR):
   ```console
   $ just bump patch     # or: minor | major
   ```
   This updates the workspace version, the internal dependency requirements,
   `Cargo.lock`, and stamps the `## [Unreleased]` CHANGELOG section, then commits
   `Release vX.Y.Z`.
2. **Open the PR and merge it** to `main` (CI must pass).
3. **Tag and trigger the publish** from an up-to-date `main`:
   ```console
   $ git switch main && git pull
   $ just publish
   ```
   `just publish` pushes the `vX.Y.Z` tag, which triggers
   [`.github/workflows/publish.yml`](../.github/workflows/publish.yml). The run
   **pauses on the `crates-io` environment for an approver**; once approved it
   verifies packaging, mints an OIDC token via
   [`rust-lang/crates-io-auth-action`](https://github.com/rust-lang/crates-io-auth-action),
   and runs `cargo publish --workspace` (crates publish in dependency order).

## One-time setup

### 1. GitHub environment (approval gate)

In GitHub → repo **Settings → Environments → New environment**, create
`crates-io` and add the release approver(s) under **Required reviewers**. The
publish job runs in this environment, so every release **pauses for a human to
approve** before any crate is published.

### 2. crates.io Trusted Publishers (per crate)

Trusted Publishing is configured **per crate** on crates.io, and a crate must
already exist to configure it. For each of the three crates, in
crates.io → the crate → **Settings → Trusted Publishing → Add**, create a GitHub
publisher:

- **Repository owner / name:** `arcanyx-pub` / `snowdrop-id-rs`
- **Workflow filename:** `publish.yml`
- **Environment:** `crates-io` (must match the workflow's `environment:`)

### Bootstrapping a brand-new crate

crates.io can only add a Trusted Publisher to a crate that already exists, so the
**first ever publish of a new crate** can't use OIDC. For `snowdrop-id-postgres`
(new in 0.3.0), do the first release once with a token, then switch to OIDC:

```console
# One time, locally, with a crates.io API token that can publish new crates:
$ CARGO_REGISTRY_TOKEN=<token> cargo publish -p snowdrop-id-postgres
```

Then add its Trusted Publisher (above). Every subsequent release — and all
releases of the two already-published crates — goes through the workflow with no
token. (If crates.io has since added support for OIDC on a crate's initial
publish, use that instead and skip the token step.)

## Notes

- `just publish` refuses to run off `main` or with a dirty tree, and errors if
  the `vX.Y.Z` tag already exists (i.e. you forgot to `just bump`).
- The tag is the source of truth for what gets published; the workflow builds
  from the tagged commit.
- To dry-run packaging locally before releasing: `just package`.
