use core::fmt;
use core::ops::Deref;
use core::str::FromStr;

use crate::base62;
use crate::epoch::Epoch;
use crate::machine::MachineId;

pub(crate) const SEQ_BITS: u32 = 22;
pub(crate) const MID_BITS: u32 = 10;
pub(crate) const TS_BITS: u32 = 31;

pub(crate) const SEQ_MASK: u64 = (1 << SEQ_BITS) - 1;
pub(crate) const MID_MASK: u64 = (1 << MID_BITS) - 1;
pub(crate) const TS_MASK: u64 = (1 << TS_BITS) - 1;

pub(crate) const MID_SHIFT: u32 = SEQ_BITS;
pub(crate) const TS_SHIFT: u32 = SEQ_BITS + MID_BITS;

/// A 63-bit Snowdrop ID.
///
/// Numeric ordering is time ordering: IDs from one generator are strictly
/// increasing, and IDs across generators are ordered by 1024 ms window.
/// The integer form always fits a non-negative `i64` (`BIGINT`).
///
/// The external form is a short base62 string via [`encode`](Self::encode) /
/// [`decode`](Self::decode), also exposed through `Display` and `FromStr`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SnowdropId(u64);

impl SnowdropId {
    /// The largest valid Snowdrop ID (`i64::MAX`).
    pub const MAX: SnowdropId = SnowdropId(i64::MAX as u64);

    /// Assembles an ID from its fields.
    ///
    /// `timestamp` is the 1024 ms window count since the epoch (31 bits) and
    /// `sequence` is the per-window counter (22 bits).
    pub const fn from_parts(
        timestamp: u32,
        machine_id: MachineId,
        sequence: u32,
    ) -> Result<SnowdropId, InvalidId> {
        if timestamp as u64 > TS_MASK {
            return Err(InvalidId::TimestampOutOfRange);
        }
        if sequence as u64 > SEQ_MASK {
            return Err(InvalidId::SequenceOutOfRange);
        }
        Ok(SnowdropId::from_parts_unchecked(
            timestamp as u64,
            machine_id.get() as u64,
            sequence as u64,
        ))
    }

    /// Assembles an ID from field values already known to be in range.
    pub(crate) const fn from_parts_unchecked(ts: u64, mid: u64, seq: u64) -> SnowdropId {
        debug_assert!(ts <= TS_MASK && mid <= MID_MASK && seq <= SEQ_MASK);
        SnowdropId((ts << TS_SHIFT) | (mid << MID_SHIFT) | seq)
    }

    /// The 31-bit timestamp field: 1024 ms windows since the epoch.
    pub const fn timestamp(self) -> u32 {
        (self.0 >> TS_SHIFT) as u32
    }

    /// The 10-bit machine ID field.
    pub const fn machine_id(self) -> MachineId {
        match MachineId::new(((self.0 >> MID_SHIFT) & MID_MASK) as u16) {
            Some(mid) => mid,
            None => unreachable!(),
        }
    }

    /// The 22-bit sequence field.
    pub const fn sequence(self) -> u32 {
        (self.0 & SEQ_MASK) as u32
    }

    /// The Unix-millisecond instant at which this ID's 1024 ms timestamp
    /// window starts, for the given epoch. Precision is 1024 ms.
    pub const fn window_start_ms(self, epoch: Epoch) -> u64 {
        epoch.unix_ms() + ((self.timestamp() as u64) << 10)
    }

    /// Returns the ID as a `u64` (bit 63 is always 0).
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// Returns the ID as a non-negative `i64`, suitable for a `BIGINT`
    /// database column.
    pub const fn as_i64(self) -> i64 {
        self.0 as i64
    }

    /// Creates an ID from its integer form, rejecting values with bit 63 set.
    pub const fn from_u64(value: u64) -> Result<SnowdropId, InvalidId> {
        if value > i64::MAX as u64 {
            return Err(InvalidId::SignBitSet);
        }
        Ok(SnowdropId(value))
    }

    /// Encodes the ID to its base62 external form (per SPEC §6.2), without
    /// allocating. At most 11 characters; 7 or fewer for the first IDs of
    /// each window.
    pub fn encode(self) -> EncodedId {
        let mut buf = [0u8; base62::MAX_LEN];
        let len = base62::encode_into(transform(self.0), &mut buf);
        EncodedId {
            buf,
            len: len as u8,
        }
    }

    /// Decodes a base62 external form back to an ID (per SPEC §6.3).
    ///
    /// Rejects empty strings, strings longer than 11 characters, characters
    /// outside the base62 alphabet, values ≥ 2⁶³, and non-canonical leading
    /// zeros.
    pub fn decode(s: &str) -> Result<SnowdropId, DecodeError> {
        Ok(SnowdropId(untransform(base62::decode_str(s)?)))
    }
}

/// The field-order-swap transform (SPEC §6.2): `(seq << 41) | (mid << 31) | ts`.
const fn transform(id: u64) -> u64 {
    let ts = id >> TS_SHIFT;
    let mid = (id >> MID_SHIFT) & MID_MASK;
    let seq = id & SEQ_MASK;
    (seq << (TS_BITS + MID_BITS)) | (mid << TS_BITS) | ts
}

const fn untransform(v: u64) -> u64 {
    let seq = v >> (TS_BITS + MID_BITS);
    let mid = (v >> TS_BITS) & MID_MASK;
    let ts = v & TS_MASK;
    (ts << TS_SHIFT) | (mid << MID_SHIFT) | seq
}

impl fmt::Display for SnowdropId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.encode())
    }
}

impl fmt::Debug for SnowdropId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SnowdropId({} \"{}\" ts={} mid={} seq={})",
            self.0,
            self.encode(),
            self.timestamp(),
            self.machine_id(),
            self.sequence()
        )
    }
}

impl FromStr for SnowdropId {
    type Err = DecodeError;

    fn from_str(s: &str) -> Result<SnowdropId, DecodeError> {
        SnowdropId::decode(s)
    }
}

impl From<SnowdropId> for u64 {
    fn from(id: SnowdropId) -> u64 {
        id.as_u64()
    }
}

impl From<SnowdropId> for i64 {
    fn from(id: SnowdropId) -> i64 {
        id.as_i64()
    }
}

impl TryFrom<u64> for SnowdropId {
    type Error = InvalidId;

    fn try_from(value: u64) -> Result<SnowdropId, InvalidId> {
        SnowdropId::from_u64(value)
    }
}

impl TryFrom<i64> for SnowdropId {
    type Error = InvalidId;

    fn try_from(value: i64) -> Result<SnowdropId, InvalidId> {
        if value < 0 {
            return Err(InvalidId::SignBitSet);
        }
        Ok(SnowdropId(value as u64))
    }
}

/// The base62 external form of a [`SnowdropId`]: an 11-byte inline buffer,
/// no allocation. Derefs to `&str`.
#[derive(Clone, Copy)]
pub struct EncodedId {
    buf: [u8; base62::MAX_LEN],
    len: u8,
}

impl EncodedId {
    /// The encoded string.
    pub fn as_str(&self) -> &str {
        let bytes = &self.buf[base62::MAX_LEN - self.len as usize..];
        // SAFETY: the buffer is filled exclusively from the base62 alphabet,
        // which is ASCII.
        unsafe { core::str::from_utf8_unchecked(bytes) }
    }
}

impl Deref for EncodedId {
    type Target = str;

    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for EncodedId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for EncodedId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for EncodedId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl PartialEq for EncodedId {
    fn eq(&self, other: &EncodedId) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for EncodedId {}

impl PartialEq<str> for EncodedId {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for EncodedId {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

/// Error constructing a [`SnowdropId`] from raw values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InvalidId {
    /// Bit 63 was set (or the `i64` was negative).
    SignBitSet,
    /// The timestamp field exceeds 31 bits.
    TimestampOutOfRange,
    /// The machine ID exceeds 1023.
    MachineIdOutOfRange,
    /// The sequence field exceeds 22 bits.
    SequenceOutOfRange,
}

impl fmt::Display for InvalidId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InvalidId::SignBitSet => f.write_str("sign bit set: not a valid 63-bit Snowdrop ID"),
            InvalidId::TimestampOutOfRange => f.write_str("timestamp exceeds 31 bits"),
            InvalidId::MachineIdOutOfRange => f.write_str("machine ID exceeds 1023"),
            InvalidId::SequenceOutOfRange => f.write_str("sequence exceeds 22 bits"),
        }
    }
}

impl std::error::Error for InvalidId {}

/// Error decoding a base62 string to a [`SnowdropId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecodeError {
    /// The string was empty.
    Empty,
    /// The string was longer than 11 characters.
    TooLong,
    /// A character outside the base62 alphabet, at byte `index`.
    InvalidCharacter {
        /// Byte offset of the offending character.
        index: usize,
    },
    /// The value does not fit in 63 bits.
    Overflow,
    /// Non-canonical form (leading zero digits).
    NonCanonical,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Empty => f.write_str("empty string"),
            DecodeError::TooLong => f.write_str("longer than 11 characters"),
            DecodeError::InvalidCharacter { index } => {
                write!(f, "invalid base62 character at byte {index}")
            }
            DecodeError::Overflow => f.write_str("value does not fit in 63 bits"),
            DecodeError::NonCanonical => f.write_str("non-canonical form (leading zeros)"),
        }
    }
}

impl std::error::Error for DecodeError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn mid(v: u16) -> MachineId {
        MachineId::new(v).unwrap()
    }

    #[test]
    fn field_roundtrip() {
        let id = SnowdropId::from_parts(46_112_984, mid(613), 12_345).unwrap();
        assert_eq!(id.timestamp(), 46_112_984);
        assert_eq!(id.machine_id(), mid(613));
        assert_eq!(id.sequence(), 12_345);
        assert_eq!(id.as_u64(), 198_053_760_772_091_961);
    }

    #[test]
    fn from_parts_validates() {
        assert_eq!(
            SnowdropId::from_parts(1 << 31, mid(0), 0),
            Err(InvalidId::TimestampOutOfRange)
        );
        assert_eq!(
            SnowdropId::from_parts(0, mid(0), 1 << 22),
            Err(InvalidId::SequenceOutOfRange)
        );
    }

    #[test]
    fn integer_conversions() {
        assert_eq!(SnowdropId::from_u64(1 << 63), Err(InvalidId::SignBitSet));
        assert_eq!(SnowdropId::try_from(-1i64), Err(InvalidId::SignBitSet));
        let id = SnowdropId::from_u64(42).unwrap();
        assert_eq!(i64::from(id), 42);
        assert_eq!(u64::from(id), 42);
        assert_eq!(SnowdropId::try_from(42i64).unwrap(), id);
    }

    #[test]
    fn window_start() {
        let id = SnowdropId::from_parts(1, mid(0), 0).unwrap();
        assert_eq!(id.window_start_ms(Epoch::DEFAULT), 1_735_689_601_024);
    }

    #[test]
    fn display_and_fromstr() {
        let id = SnowdropId::from_parts(46_112_984, mid(0), 0).unwrap();
        assert_eq!(id.to_string(), "37U5o");
        assert_eq!("37U5o".parse::<SnowdropId>().unwrap(), id);
        assert_eq!(id.encode(), "37U5o");
        assert_eq!(id.encode().len(), 5);
    }

    #[test]
    fn ordering_is_numeric() {
        let a = SnowdropId::from_parts(100, mid(3), 7).unwrap();
        let b = SnowdropId::from_parts(100, mid(3), 8).unwrap();
        let c = SnowdropId::from_parts(101, mid(0), 0).unwrap();
        assert!(a < b && b < c);
    }
}
