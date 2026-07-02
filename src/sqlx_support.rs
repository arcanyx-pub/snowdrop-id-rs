//! `sqlx` support: [`SnowdropId`] maps to `BIGINT` (`i64`).
//!
//! Enabled per database via the `sqlx-postgres`, `sqlx-mysql`, and
//! `sqlx-sqlite` features. Decoding rejects negative values, which cannot
//! be valid Snowdrop IDs.

use crate::id::SnowdropId;

macro_rules! impl_sqlx_for {
    ($db:ty) => {
        impl sqlx::Type<$db> for SnowdropId {
            fn type_info() -> <$db as sqlx::Database>::TypeInfo {
                <i64 as sqlx::Type<$db>>::type_info()
            }

            fn compatible(ty: &<$db as sqlx::Database>::TypeInfo) -> bool {
                <i64 as sqlx::Type<$db>>::compatible(ty)
            }
        }

        impl<'q> sqlx::Encode<'q, $db> for SnowdropId {
            fn encode_by_ref(
                &self,
                buf: &mut <$db as sqlx::Database>::ArgumentBuffer<'q>,
            ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
                <i64 as sqlx::Encode<'q, $db>>::encode_by_ref(&self.as_i64(), buf)
            }
        }

        impl<'r> sqlx::Decode<'r, $db> for SnowdropId {
            fn decode(
                value: <$db as sqlx::Database>::ValueRef<'r>,
            ) -> Result<SnowdropId, sqlx::error::BoxDynError> {
                let raw = <i64 as sqlx::Decode<'r, $db>>::decode(value)?;
                Ok(SnowdropId::try_from(raw)?)
            }
        }
    };
}

#[cfg(feature = "sqlx-postgres")]
impl_sqlx_for!(sqlx::Postgres);

#[cfg(feature = "sqlx-mysql")]
impl_sqlx_for!(sqlx::MySql);

#[cfg(feature = "sqlx-sqlite")]
impl_sqlx_for!(sqlx::Sqlite);
