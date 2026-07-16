# snowdrop-id-postgres

[![crates.io](https://img.shields.io/crates/v/snowdrop-id-postgres.svg)](https://crates.io/crates/snowdrop-id-postgres)
[![docs.rs](https://img.shields.io/docsrs/snowdrop-id-postgres)](https://docs.rs/snowdrop-id-postgres)

Postgres-backed **machine-ID leasing** for [`snowdrop-id`](https://crates.io/crates/snowdrop-id).

A [Snowdrop ID](https://crates.io/crates/snowdrop-id) generator stamps a 10-bit
machine ID into every ID, and every concurrently active generator in an ID
space must use a distinct one. This crate leases those machine IDs from a small
Postgres table instead of assigning them statically: a worker claims the lowest
free machine ID, a background task heartbeats to hold the lease, and the machine
ID is released on drop (or reclaimed after its deadline if the process dies).

Every operation is a single pooled statement with no session state, so it works
through an ordinary `PgPool` under any PgBouncer pooling mode and survives a
primary failover. A generator refuses to issue IDs while it cannot prove its
lease is still held, so no two live workers ever share a machine ID.

```rust
use snowdrop_id_postgres::{PgIdGenerator, PgMachineIdLease};
use sqlx::PgPool;

let pool = PgPool::connect("postgres://…").await?;

// The lease table (`public.snowdrop_machine_id_leases` by default) must exist
// first. Provision it in your migrations…
sqlx::raw_sql(PgMachineIdLease::schema_sql()).execute(&pool).await?;  // idempotent DDL
sqlx::raw_sql(PgMachineIdLease::seeding_sql()).execute(&pool).await?; // idempotent seed
// …or opt into automatic creation with `.builder(pool).auto_provision(true)`.

let generator = PgIdGenerator::new(pool).await?; // claims the lowest free machine ID
let id = generator.generate()?;
```

## Provisioning

Creating the table is **not** automatic by default — it needs DDL rights (and,
for a custom schema, `CREATE` on the database) that many production roles lack:

- **Out-of-the-box:** `auto_provision(true)` creates the schema, table, and seed
  rows on boot, race-safely across many instances starting at once.
- **Migrations:** run `schema_sql()` (DDL) then `seeding_sql()` (seed) once; both
  are idempotent.
- **Declarative (e.g. Atlas):** feed `schema_sql()`'s output to your tool as the
  desired schema, and run `seeding_sql()` as a deploy step (such tools don't
  manage row data).

The lease lives in `public` by default; `schema_name("…")` puts it in a
dedicated (quoted) schema for an isolated ID space. See
[`docs/pg-machine-id-leasing.md`](https://github.com/arcanyx-pub/snowdrop-id-rs/blob/main/docs/pg-machine-id-leasing.md)
for the full design.

## License

MIT
