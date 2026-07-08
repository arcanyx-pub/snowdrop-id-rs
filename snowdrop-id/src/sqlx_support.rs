//! `sqlx` support: [`SnowdropId`] maps to a 64-bit integer column.
//!
//! | Feature | Database | Column type | Rust type |
//! |---------|----------|-------------|-----------|
//! | `sqlx-postgres` | PostgreSQL | `BIGINT` | `i64` |
//! | `sqlx-mysql` | MySQL | `BIGINT` (signed) | `i64` |
//! | `sqlx-mysql-u64` | MySQL | `BIGINT UNSIGNED` | `u64` |
//! | `sqlx-sqlite` | SQLite | `BIGINT` | `i64` |
//!
//! Decoding rejects values outside the valid 63-bit Snowdrop range
//! (negative `i64`s; `u64`s with bit 63 set).
//!
//! `sqlx-mysql` and `sqlx-mysql-u64` are mutually exclusive: sqlx maps a
//! Rust type to exactly one MySQL signedness (`i64` decodes only from
//! signed columns, `u64` only from `UNSIGNED` ones), so one binary can use
//! only one column type for [`SnowdropId`]. `sqlx-mysql-u64` exists for
//! drop-in compatibility with existing unsigned MySQL schemas.

use crate::id::SnowdropId;

#[cfg(all(feature = "sqlx-mysql", feature = "sqlx-mysql-u64"))]
compile_error!(
    "the `sqlx-mysql` (signed BIGINT / i64) and `sqlx-mysql-u64` (BIGINT UNSIGNED / u64) \
     features are mutually exclusive: sqlx maps each Rust type to exactly one MySQL \
     signedness, so a binary can only use one column type for SnowdropId. Enable exactly \
     one of them (check `cargo tree -e features` to find which dependency enables the other)."
);

macro_rules! impl_sqlx_for {
    ($db:ty, $int:ty, $to_int:ident) => {
        impl sqlx::Type<$db> for SnowdropId {
            fn type_info() -> <$db as sqlx::Database>::TypeInfo {
                <$int as sqlx::Type<$db>>::type_info()
            }

            fn compatible(ty: &<$db as sqlx::Database>::TypeInfo) -> bool {
                <$int as sqlx::Type<$db>>::compatible(ty)
            }
        }

        impl<'q> sqlx::Encode<'q, $db> for SnowdropId {
            fn encode_by_ref(
                &self,
                buf: &mut <$db as sqlx::Database>::ArgumentBuffer<'q>,
            ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
                <$int as sqlx::Encode<'q, $db>>::encode_by_ref(&self.$to_int(), buf)
            }
        }

        impl<'r> sqlx::Decode<'r, $db> for SnowdropId {
            fn decode(
                value: <$db as sqlx::Database>::ValueRef<'r>,
            ) -> Result<SnowdropId, sqlx::error::BoxDynError> {
                let raw = <$int as sqlx::Decode<'r, $db>>::decode(value)?;
                Ok(SnowdropId::try_from(raw)?)
            }
        }
    };
}

#[cfg(feature = "sqlx-postgres")]
impl_sqlx_for!(sqlx::Postgres, i64, as_i64);

#[cfg(all(feature = "sqlx-mysql", not(feature = "sqlx-mysql-u64")))]
impl_sqlx_for!(sqlx::MySql, i64, as_i64);

#[cfg(all(feature = "sqlx-mysql-u64", not(feature = "sqlx-mysql")))]
impl_sqlx_for!(sqlx::MySql, u64, as_u64);

#[cfg(feature = "sqlx-sqlite")]
impl_sqlx_for!(sqlx::Sqlite, i64, as_i64);
