# snowdrop-id-rs

[![CI](https://github.com/arcanyx-pub/snowdrop-id-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/arcanyx-pub/snowdrop-id-rs/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/snowdrop-id.svg)](https://crates.io/crates/snowdrop-id)
[![docs.rs](https://img.shields.io/docsrs/snowdrop-id)](https://docs.rs/snowdrop-id)

Rust implementation of Snowdrop ID, a smaller, cuter alternative to Snowflake.

<p align="center">
  <img src="assets/snowdrop.jpg" alt="Snowdrop, the Heeler puppy mascot, holding a snowdrop flower in a snowy forest" width="600">
</p>

A Snowdrop ID is a 63-bit, roughly monotonic, collision-free identifier —
like a Snowflake ID — that additionally encodes to a very short base62
string (7 characters or fewer in the common case, as few as 5) and
interleaves exactly with Snowflake IDs in the same BTree keyspace.

See [SPEC.md](SPEC.md) for the format, generation algorithm, encoding,
and test vectors.

## Library

```rust
use snowdrop_id::{Generator, MachineId, SnowdropId};

let generator = Generator::new(MachineId::new(0).unwrap());
let id = generator.generate()?;

println!("{id}");            // "37mXl" — short base62 form
let n: i64 = id.as_i64();    // BIGINT-safe integer form
let back: SnowdropId = "37mXl".parse()?;
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

### Feature flags

| Feature | Adds |
|---------|------|
| `tokio` | `Generator::generate_async()` |
| `serde` | `Serialize`/`Deserialize` as the base62 string; numeric via `serde_u64` |
| `sqlx-postgres`, `sqlx-mysql`, `sqlx-sqlite` | `sqlx` `Type`/`Encode`/`Decode` as `BIGINT` |

## CLI

The `snowdrop` tool lives in the companion
[`snowdrop-id-cli`](snowdrop-id-cli/) crate:

```console
$ cargo install snowdrop-id-cli
$ snowdrop generate -n 2
198358378861297664	37mXl
198358378861297665	ciYFJPE
$ snowdrop decode 37mXl
id:           198358378756440064
hex:          0x02c0b5e500000000
base62:       37mXl
timestamp:    46183909
machine-id:   0
sequence:     0
window-start: 2026-07-02T08:45:22.816Z (1782981922816 ms, epoch 1735689600000 ms)
$ snowdrop encode 198358378756440064
37mXl
```

## License

MIT
