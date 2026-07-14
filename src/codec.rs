use crate::permute::MAX_ID;

const ALPHABET: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
pub const CODE_LEN: usize = 7;

/// Encodes a 40-bit integer (<= MAX_ID) into a 7-character base62 string.
/// Note: values `n > MAX_ID` are silently truncated (lossy).
pub fn to_base62(mut n: u64) -> String {
    let mut buf = [b'0'; CODE_LEN];
    let mut i = CODE_LEN;
    while n > 0 && i > 0 {
        i -= 1;
        buf[i] = ALPHABET[(n % 62) as usize];
        n /= 62;
    }
    String::from_utf8(buf.to_vec()).expect("alphabet is ASCII")
}

fn val(c: u8) -> Option<u64> {
    match c {
        b'0'..=b'9' => Some((c - b'0') as u64),
        b'A'..=b'Z' => Some((c - b'A' + 10) as u64),
        b'a'..=b'z' => Some((c - b'a' + 36) as u64),
        _ => None,
    }
}

pub fn from_base62(s: &str) -> Option<u64> {
    if s.len() != CODE_LEN {
        return None;
    }
    let mut n: u64 = 0;
    for &c in s.as_bytes() {
        let d = val(c)?;
        n = n.checked_mul(62)?.checked_add(d)?;
    }
    if n <= MAX_ID {
        Some(n)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_base62() {
        for n in [0u64, 1, 61, 62, 3843, 1_000_000, (1u64 << 40) - 1] {
            let s = to_base62(n);
            assert_eq!(s.len(), CODE_LEN);
            assert_eq!(from_base62(&s), Some(n));
        }
    }

    #[test]
    fn rejects_invalid_char() {
        assert_eq!(from_base62("!!!!!!!"), None);
    }

    #[test]
    fn rejects_wrong_size() {
        assert_eq!(from_base62("abc"), None);
        assert_eq!(from_base62("aaaaaaaaaaaa"), None);
    }

    #[test]
    fn rejects_out_of_range() {
        assert_eq!(from_base62("zzzzzzz"), None);
        let max = (1u64 << 40) - 1;
        assert_eq!(from_base62(&to_base62(max)), Some(max));
    }
}
