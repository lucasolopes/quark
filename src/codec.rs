use crate::permute::MAX_ID;

const ALPHABET: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
pub const CODE_LEN: usize = 7;

/// Codifica um inteiro de 40 bits (≤ MAX_ID) em uma string base62 de 7 caracteres.
/// Nota: valores `n > MAX_ID` são truncados silenciosamente (lossy).
pub fn to_base62(mut n: u64) -> String {
    let mut buf = [b'0'; CODE_LEN];
    let mut i = CODE_LEN;
    while n > 0 && i > 0 {
        i -= 1;
        buf[i] = ALPHABET[(n % 62) as usize];
        n /= 62;
    }
    // n==0 já fica preenchido com '0'; ordem correta pois preenchemos do fim.
    String::from_utf8(buf.to_vec()).expect("alfabeto é ASCII")
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
    fn rejeita_char_invalido() {
        assert_eq!(from_base62("!!!!!!!"), None);
    }

    #[test]
    fn rejeita_tamanho_errado() {
        assert_eq!(from_base62("abc"), None);
        assert_eq!(from_base62("aaaaaaaaaaaa"), None);
    }

    #[test]
    fn rejeita_fora_do_range() {
        // "zzzzzzz" decodifica para > 2^40-1, deve ser rejeitado
        assert_eq!(from_base62("zzzzzzz"), None);
        // maior valor válido ainda é aceito
        let max = (1u64 << 40) - 1;
        assert_eq!(from_base62(&to_base62(max)), Some(max));
    }
}
