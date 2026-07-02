use crate::id::DecodeError;

/// Base62 alphabet in ascending digit-value order (ASCII order), per §6.1.
pub(crate) const ALPHABET: &[u8; 62] =
    b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Maximum encoded length: 2^63 − 1 needs 11 base62 digits.
pub(crate) const MAX_LEN: usize = 11;

const INVALID: u8 = 0xFF;

static DECODE_TABLE: [u8; 256] = {
    let mut table = [INVALID; 256];
    let mut i = 0;
    while i < 62 {
        table[ALPHABET[i] as usize] = i as u8;
        i += 1;
    }
    table
};

/// Encodes `v` into `buf` right-aligned; returns the number of digits.
/// The digits occupy `buf[MAX_LEN - len..]`.
pub(crate) fn encode_into(mut v: u64, buf: &mut [u8; MAX_LEN]) -> usize {
    if v == 0 {
        buf[MAX_LEN - 1] = b'0';
        return 1;
    }
    let mut i = MAX_LEN;
    while v != 0 {
        i -= 1;
        buf[i] = ALPHABET[(v % 62) as usize];
        v /= 62;
    }
    MAX_LEN - i
}

/// Decodes a canonical base62 string to a value in `0..2^63`, per §6.3.
pub(crate) fn decode_str(s: &str) -> Result<u64, DecodeError> {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return Err(DecodeError::Empty);
    }
    if bytes.len() > MAX_LEN {
        return Err(DecodeError::TooLong);
    }
    if bytes.len() > 1 && bytes[0] == b'0' {
        return Err(DecodeError::NonCanonical);
    }
    let mut v: u64 = 0;
    for (index, &b) in bytes.iter().enumerate() {
        let digit = DECODE_TABLE[b as usize];
        if digit == INVALID {
            return Err(DecodeError::InvalidCharacter { index });
        }
        v = v
            .checked_mul(62)
            .and_then(|v| v.checked_add(digit as u64))
            .ok_or(DecodeError::Overflow)?;
    }
    if v > i64::MAX as u64 {
        return Err(DecodeError::Overflow);
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(v: u64) -> String {
        let mut buf = [0u8; MAX_LEN];
        let len = encode_into(v, &mut buf);
        String::from_utf8(buf[MAX_LEN - len..].to_vec()).unwrap()
    }

    #[test]
    fn zero_and_digits() {
        assert_eq!(encode(0), "0");
        assert_eq!(encode(9), "9");
        assert_eq!(encode(10), "A");
        assert_eq!(encode(35), "Z");
        assert_eq!(encode(36), "a");
        assert_eq!(encode(61), "z");
        assert_eq!(encode(62), "10");
    }

    #[test]
    fn max_value() {
        let max = i64::MAX as u64;
        assert_eq!(encode(max), "AzL8n0Y58m7");
        assert_eq!(decode_str("AzL8n0Y58m7").unwrap(), max);
    }

    #[test]
    fn rejects_empty_and_long() {
        assert_eq!(decode_str(""), Err(DecodeError::Empty));
        assert_eq!(decode_str("111111111111"), Err(DecodeError::TooLong));
    }

    #[test]
    fn rejects_invalid_characters() {
        assert_eq!(
            decode_str("ab-c"),
            Err(DecodeError::InvalidCharacter { index: 2 })
        );
        assert_eq!(
            decode_str("é"),
            Err(DecodeError::InvalidCharacter { index: 0 })
        );
    }

    #[test]
    fn rejects_overflow() {
        // 2^63 exactly, one above SnowdropId range.
        assert_eq!(decode_str("AzL8n0Y58m8"), Err(DecodeError::Overflow));
        // Largest 11-digit string, overflows u64 mid-parse.
        assert_eq!(decode_str("zzzzzzzzzzz"), Err(DecodeError::Overflow));
    }

    #[test]
    fn rejects_non_canonical() {
        assert_eq!(decode_str("0"), Ok(0));
        assert_eq!(decode_str("01"), Err(DecodeError::NonCanonical));
        assert_eq!(decode_str("00"), Err(DecodeError::NonCanonical));
    }

    #[test]
    fn roundtrip_sweep() {
        // Deterministic LCG sweep; no external rand dependency.
        let mut x: u64 = 0x243F6A8885A308D3;
        for _ in 0..10_000 {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let v = x >> 1; // 63-bit
            assert_eq!(decode_str(&encode(v)).unwrap(), v);
        }
    }
}
