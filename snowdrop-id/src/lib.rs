//! Snowdrop IDs: 63-bit, roughly monotonic, collision-free identifiers —
//! like Snowflake IDs — that additionally encode to a very short base62
//! string and interleave exactly with Snowflake IDs in the same keyspace.
//!
//! See `SPEC.md` in the repository for the full format specification.
//!
//! # Quick start
//!
//! ```
//! use snowdrop_id::{IdGenerator, MachineId, Id};
//!
//! let generator = IdGenerator::new(MachineId::new(0).unwrap());
//! let id = generator.generate().unwrap();
//!
//! // Short base62 external form; round-trips losslessly.
//! let s = id.encode();
//! assert_eq!(s.parse::<Id>().unwrap(), id);
//!
//! // BIGINT-safe integer form; numeric order is time order.
//! let n: i64 = id.as_i64();
//! assert!(n >= 0);
//! ```
//!
//! # Feature flags
//!
//! - `tokio` — adds [`IdGenerator::generate_async`], which awaits instead of
//!   blocking the thread on (rare) sequence exhaustion.
//! - `serde` — `Serialize`/`Deserialize` for [`Id`] as the base62
//!   string; numeric opt-in via [`serde_u64`].
//! - `sqlx-postgres`, `sqlx-mysql`, `sqlx-sqlite` — `sqlx` `Type`/`Encode`/
//!   `Decode` impls mapping [`Id`] to `BIGINT`.
//! - `postgres-machine-id` — machine IDs leased from a Postgres table
//!   ([`PgMachineIdLease`], [`PgIdGenerator`]), for clusters with no static
//!   machine-ID assignment; pooler- and failover-safe.
//!
//! A companion command-line tool is available as the `snowdrop-id-cli`
//! crate (`cargo install snowdrop-id-cli`).
//!
//! For retrofits where injecting an [`IdGenerator`] isn't practical, the
//! [`global`] module provides a process-global generator configured once
//! at startup.

#![warn(missing_docs)]
#![forbid(unsafe_code)]

mod base62;
mod clock;
mod epoch;
mod generator;
pub mod global;
mod id;
mod machine;

#[cfg(feature = "serde")]
mod serde_support;

#[cfg(any(
    feature = "sqlx-postgres",
    feature = "sqlx-mysql",
    feature = "sqlx-sqlite"
))]
mod sqlx_support;

#[cfg(feature = "postgres-machine-id")]
mod pg_machine_id;

pub use clock::{Clock, SystemClock};
pub use epoch::Epoch;
pub use generator::{GenerateError, IdGenerator, IdGeneratorBuilder, TryGenerateError};
pub use id::{DecodeError, EncodedId, Id, InvalidId};
pub use machine::MachineId;

#[cfg(feature = "serde")]
pub use serde_support::serde_u64;

#[cfg(feature = "postgres-machine-id")]
pub use pg_machine_id::{
    DEFAULT_TABLE, PgGenerateError, PgIdGenerator, PgIdGeneratorBuilder, PgLeaseError,
    PgMachineIdLease, PgMachineIdLeaseBuilder,
};
