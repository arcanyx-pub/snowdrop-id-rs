# Changelog

Notable changes to the `snowdrop-id` and `snowdrop-id-cli` crates (versioned
in lockstep). Format follows [Keep a Changelog](https://keepachangelog.com);
versions follow [SemVer](https://semver.org).

## [0.2.1] - 2026-07-14

### Fixed

- Refreshed `Cargo.lock` to replace the yanked `spin` 0.9.8 — a transitive
  dependency via `flume` / `sqlx-sqlite` — with 0.9.9. No API or behavior
  changes.

## [0.2.0] - 2026-07-11

### Breaking

- Renamed `SnowdropId` to `Id`, and `Generator` / `GeneratorBuilder` to
  `IdGenerator` / `IdGeneratorBuilder`.
- Changed the default epoch from 2025-01-01 to **2026-01-01T00:00:00Z**
  (`1767225600000` ms). IDs generated under the old default decode to
  different wall-clock times under the new one. Spec bumped to v1.0 draft 3.
- The `sqlx-*` features now target sqlx **0.9** (a public dependency of
  those features; sqlx 0.8 users should stay on snowdrop-id 0.1.x until
  they upgrade). Core MSRV stays 1.85; the sqlx-backed features require
  Rust 1.94+ via sqlx.

### Added

- `postgres-machine-id` feature: machine IDs leased from a Postgres table,
  for clusters with no static machine-ID assignment. `PgMachineIdLease`
  (guard) and `PgIdGenerator` (bundled generator) claim the lowest free ID
  through a connection pool, heartbeat to hold the lease, and release it on
  drop; a generator stops issuing IDs while its lease is not confirmed held,
  so no two live workers share a machine ID. Every operation is a single
  pooled statement, so it works under any PgBouncer pooling mode and survives
  a primary failover. See `docs/pg-machine-id-leasing.md`.
- This changelog, and a bit-layout diagram in the README.

## [0.1.2] - 2026-07-08

### Fixed

- Mascot image no longer 404s on the crates.io page (absolute URL).

## [0.1.1] - 2026-07-08

### Fixed

- docs.rs now documents all features; the 0.1.0 docs were missing
  `generate_async`, `serde_u64`, and the sqlx impls.

## [0.1.0] - 2026-07-08

### Added

- Initial release: Snowdrop ID specification (v1.0 draft 2), `snowdrop-id`
  library (lock-free generator, short base62 encoding, `tokio` / `serde` /
  `sqlx-postgres` / `sqlx-mysql` / `sqlx-sqlite` features, process-global
  generator), and the `snowdrop` CLI (`snowdrop-id-cli`).
