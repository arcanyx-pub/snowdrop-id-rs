//! Snowdrop IDs: 63-bit, roughly monotonic, collision-free identifiers —
//! like Snowflake IDs — that additionally encode to a very short base62
//! string and interleave exactly with Snowflake IDs in the same keyspace.
//!
//! See `SPEC.md` in the repository for the full format specification.
//!
//! # Quick start
//!
//! ```
//! use snowdrop_id::{Generator, MachineId, SnowdropId};
//!
//! let generator = Generator::new(MachineId::new(0).unwrap());
//! let id = generator.generate().unwrap();
//!
//! // Short base62 external form; round-trips losslessly.
//! let s = id.encode();
//! assert_eq!(s.parse::<SnowdropId>().unwrap(), id);
//!
//! // BIGINT-safe integer form; numeric order is time order.
//! let n: i64 = id.as_i64();
//! assert!(n >= 0);
//! ```
//!
//! # Feature flags
//!
//! - `tokio` — adds [`Generator::generate_async`], which awaits instead of
//!   blocking the thread on (rare) sequence exhaustion.
//! - `serde` — `Serialize`/`Deserialize` for [`SnowdropId`] as the base62
//!   string; numeric opt-in via [`serde_u64`].
//! - `sqlx-postgres`, `sqlx-mysql`, `sqlx-sqlite` — `sqlx` `Type`/`Encode`/
//!   `Decode` impls mapping [`SnowdropId`] to `BIGINT`.
//! - `cli` — builds the `snowdrop` command-line tool
//!   (`cargo install snowdrop-id --features cli`).

#![warn(missing_docs)]
#![forbid(unsafe_code)]

mod base62;
mod clock;
mod epoch;
mod generator;
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

pub use clock::{Clock, SystemClock};
pub use epoch::Epoch;
pub use generator::{GenerateError, Generator, GeneratorBuilder, TryGenerateError};
pub use id::{DecodeError, EncodedId, InvalidId, SnowdropId};
pub use machine::MachineId;

#[cfg(feature = "serde")]
pub use serde_support::serde_u64;
