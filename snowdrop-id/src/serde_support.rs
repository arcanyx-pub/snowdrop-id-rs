//! `Serialize`/`Deserialize` for [`Id`].
//!
//! The default representation is the base62 string: it is short, and the
//! integer form exceeds 2⁵³, which JavaScript JSON consumers silently
//! corrupt. Use [`serde_u64`] for a numeric representation.

use core::fmt;

use serde::de::{Deserializer, Error as DeError, Unexpected, Visitor};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use crate::id::Id;

impl Serialize for Id {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.encode())
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Id, D::Error> {
        struct Base62Visitor;

        impl Visitor<'_> for Base62Visitor {
            type Value = Id;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a base62 Snowdrop ID string")
            }

            fn visit_str<E: DeError>(self, s: &str) -> Result<Id, E> {
                Id::decode(s).map_err(|_| E::invalid_value(Unexpected::Str(s), &self))
            }
        }

        deserializer.deserialize_str(Base62Visitor)
    }
}

/// Serialize a [`Id`] as its `u64` integer form instead of the
/// base62 string: `#[serde(with = "snowdrop_id::serde_u64")]`.
///
/// Prefer the default string form for JSON that JavaScript may consume.
pub mod serde_u64 {
    use super::*;

    /// Serializes the ID as a `u64`.
    pub fn serialize<S: Serializer>(id: &Id, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(id.as_u64())
    }

    /// Deserializes the ID from an unsigned integer, rejecting values with
    /// bit 63 set.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Id, D::Error> {
        let value = u64::deserialize(deserializer)?;
        Id::from_u64(value).map_err(|_| {
            D::Error::invalid_value(Unexpected::Unsigned(value), &"a 63-bit Snowdrop ID")
        })
    }
}
