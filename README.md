# snowdrop-id-rs

[![CI](https://github.com/arcanyx-pub/snowdrop-id-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/arcanyx-pub/snowdrop-id-rs/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/snowdrop-id.svg)](https://crates.io/crates/snowdrop-id)
[![docs.rs](https://img.shields.io/docsrs/snowdrop-id)](https://docs.rs/snowdrop-id)

Rust implementation of Snowdrop ID, a smaller, cuter alternative to Snowflake.

<p align="center">
  <!-- Absolute URL: crates.io resolves relative paths against the crate's
       workspace subdirectory (path_in_vcs), not the repo root, and 404s. -->
  <img src="https://raw.githubusercontent.com/arcanyx-pub/snowdrop-id-rs/main/assets/snowdrop.jpg" alt="Snowdrop, the Heeler puppy mascot, holding a snowdrop flower in a snowy forest" width="600">
</p>

TL;DR:
```text
DB: 200708872623620096
URL: example.com/user/3A4ue
```

A Snowdrop ID is a 63-bit, roughly monotonic, collision-free identifier —
like a Snowflake ID — that additionally encodes to a very short base62
string (7 characters or fewer in the common case, as few as 5) and
interleaves exactly with Snowflake IDs in the same BTree keyspace.

## ID format

```text
 63  62                            32  31         22  21                     0
┌───┬────────────────────────────────┬───────────────┬───────────────────────┐
│ 0 │ timestamp — 31 bits            │ machine ID    │ sequence — 22 bits    │
│   │ 1024 ms windows since epoch    │ 10 bits       │ per-window counter    │
└───┴────────────────────────────────┴───────────────┴───────────────────────┘
```

The base62 string form encodes the same 63 bits with the fields in swapped
order — sequence, machine ID, timestamp — so the high bits are usually zero
and the string stays short.

See [SPEC.md](SPEC.md) for the full format, generation algorithm, encoding,
and test vectors.

## Library

```rust
use snowdrop_id::{Id, IdGenerator, MachineId};

let generator = IdGenerator::new(MachineId::new(0).unwrap());
let id = generator.generate()?;

println!("{id}");            // "37mXl" — short base62 form
let n: i64 = id.as_i64();    // BIGINT-safe integer form
let back: Id = "37mXl".parse()?;
```

The generator is lock-free and `Send + Sync` — share one per process via
`Arc` or a `static`. With the `tokio` feature, `generate_async()` awaits
instead of blocking on the (rare) sequence-exhaustion wait.

For retrofits where injecting a generator isn't practical, configure a
process-global one once at startup:

```rust
snowdrop_id::global::init(MachineId::new(0).unwrap())?; // in main()
let id = snowdrop_id::global::generate()?;              // anywhere else
```

With the `postgres-machine-id` feature, clusters can lease machine IDs
from Postgres advisory locks instead of assigning them statically — the
lock is held by a dedicated connection, re-acquired automatically after
connection loss, and released if the process dies:

```rust
let generator = PgIdGenerator::acquire("postgres://…".parse()?).await?;
let id = generator.generate()?;
```

### Feature flags

| Feature | Adds |
|---------|------|
| `tokio` | `IdGenerator::generate_async()` |
| `serde` | `Serialize`/`Deserialize` as the base62 string; numeric via `serde_u64` |
| `sqlx-postgres`, `sqlx-mysql`, `sqlx-sqlite` | `sqlx` `Type`/`Encode`/`Decode` as `BIGINT` |
| `postgres-machine-id` | machine IDs leased via Postgres advisory locks |

The core crate and CLI support Rust 1.85+; the sqlx-backed features
(`sqlx-*`, `postgres-machine-id`) require Rust 1.94+ via sqlx 0.9.

## CLI

The `snowdrop` tool lives in the companion
[`snowdrop-id-cli`](snowdrop-id-cli/) crate:

```console
$ cargo install snowdrop-id-cli
$ snowdrop generate -n 2
69665877074640896	163eZ
69665877074640897	ciLhXHb
$ snowdrop decode 163eZ
id:           69665877074640896
hex:          0x00f780bf00000000
base62:       163eZ
timestamp:    16220351
machine-id:   0
sequence:     0
window-start: 2026-07-12T05:47:19.424Z (1783835239424 ms, epoch 1767225600000 ms)
$ snowdrop encode 69665877074640896
163eZ
```

## License

MIT
