# Changelog

Notable changes to the `snowdrop-id` and `snowdrop-id-cli` crates (versioned
in lockstep). Format follows [Keep a Changelog](https://keepachangelog.com);
versions follow [SemVer](https://semver.org).

## [0.2.0] - 2026-07-11

### Breaking

- Renamed `SnowdropId` to `Id`, and `Generator` / `GeneratorBuilder` to
  `IdGenerator` / `IdGeneratorBuilder`.
- Changed the default epoch from 2025-01-01 to **2026-01-01T00:00:00Z**
  (`1767225600000` ms). IDs generated under the old default decode to
  different wall-clock times under the new one. Spec bumped to v1.0 draft 3.

### Added

- `postgres-machine-id` feature: machine IDs leased via Postgres advisory
  locks. `PgMachineIdLease` (guard) and `PgIdGenerator` (bundled generator)
  with connection keepalive, automatic lock re-acquisition, and a
  configurable `LeaseLossPolicy` (default: `Poison`).
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
