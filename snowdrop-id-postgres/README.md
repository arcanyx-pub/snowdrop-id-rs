# snowdrop-id-postgres

[![crates.io](https://img.shields.io/crates/v/snowdrop-id-postgres.svg)](https://crates.io/crates/snowdrop-id-postgres)
[![docs.rs](https://img.shields.io/docsrs/snowdrop-id-postgres)](https://docs.rs/snowdrop-id-postgres)

Postgres-backed **machine-ID leasing** for [`snowdrop-id`](https://crates.io/crates/snowdrop-id).

A [Snowdrop ID](https://crates.io/crates/snowdrop-id) generator stamps a 10-bit
machine ID into every ID, and every concurrently active generator in an ID
space must use a distinct one. This crate leases those IDs from a small
Postgres table instead of assigning them statically: a worker claims the lowest
free machine ID, a background task heartbeats to hold the lease, and the ID is
released on drop (or reclaimed after its deadline if the process dies).

Every operation is a single pooled statement with no session state, so it works
through an ordinary `PgPool` under any PgBouncer pooling mode and survives a
primary failover. A generator refuses to issue IDs while it cannot prove its
lease is still held, so no two live workers ever share a machine ID.

```rust
use snowdrop_id_postgres::{PgIdGenerator, PgMachineIdLease};
use sqlx::PgPool;

let pool = PgPool::connect("postgres://…").await?;

// The lease table (`snowdrop.machine_id_leases` by default) must exist first.
// Run this in your migrations…
sqlx::raw_sql(&PgMachineIdLease::schema_sql())
    .execute(&pool)
    .await?;
// …or opt into automatic creation with `.builder(pool).auto_create(true)`.

let generator = PgIdGenerator::acquire(pool).await?; // claims the lowest free ID
let id = generator.generate()?;
```

Creating the schema and table is **not** automatic by default: it needs DDL
privileges that many production roles lack. See
[`docs/pg-machine-id-leasing.md`](https://github.com/arcanyx-pub/snowdrop-id-rs/blob/main/docs/pg-machine-id-leasing.md)
in the repository for the full design.

## License

MIT
