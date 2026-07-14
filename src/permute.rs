//! Bijection over [0, 2^WIDTH_BITS) via a balanced Feistel network
//! with an ARX round function. It is format-preserving: encode/decode never
//! leave the range, never collide.

pub const WIDTH_BITS: u32 = 40;
/// Calibrated (see bin/calibrate): the smallest number of rounds with exact 0.5000
/// avalanche and full 40/40 coverage (diffusion closed); 3 rounds only reaches 0.4866.
pub const ROUNDS: usize = 4;
pub const MAX_ID: u64 = (1u64 << WIDTH_BITS) - 1;

/// Derives the round subkey from the master key and the round index.
#[inline]
fn subkey(key: u64, round: usize) -> u32 {
    let x = key.rotate_left((round as u32) * 7 + 1)
        ^ (0x9E3779B97F4A7C15u64.wrapping_mul(round as u64 + 1));
    (x ^ (x >> 32)) as u32
}

/// ARX round function: mixes one half (half_bits) with the subkey.
#[inline]
fn round_fn(r: u32, key: u64, round: usize, half_bits: u32) -> u32 {
    let mask = (1u32 << half_bits) - 1;
    let rk = subkey(key, round);
    let mut x = r.wrapping_add(rk);
    x ^= x.rotate_left(7);
    x = x.wrapping_add(x.rotate_left(13));
    x ^= x.rotate_left(17);
    x & mask
}

/// Generic Feistel over `width` bits (width even), parameterized by the number of
/// rounds. Generalizes the Feistel used by encode; also used by the small-width
/// bijectivity test and by the calibration harness (bin/calibrate),
/// which measures the real permutation (no divergent copy).
#[inline]
pub fn feistel_n(input: u64, key: u64, rounds: usize, width: u32) -> u64 {
    let half = width / 2;
    let mask = (1u64 << half) - 1;
    let mut l = ((input >> half) & mask) as u32;
    let mut r = (input & mask) as u32;
    for round in 0..rounds {
        let f = round_fn(r, key, round, half);
        let new_l = r;
        let new_r = l ^ f;
        l = new_l;
        r = new_r;
    }
    ((l as u64) << half) | (r as u64)
}

#[inline]
pub fn feistel_n_inv(input: u64, key: u64, rounds: usize, width: u32) -> u64 {
    let half = width / 2;
    let mask = (1u64 << half) - 1;
    let mut l = ((input >> half) & mask) as u32;
    let mut r = (input & mask) as u32;
    for round in (0..rounds).rev() {
        let r_prev = l;
        let f = round_fn(r_prev, key, round, half);
        let l_prev = r ^ f;
        l = l_prev;
        r = r_prev;
    }
    ((l as u64) << half) | (r as u64)
}

/// Encodes an id in [0, MAX_ID] via Feistel. Input outside the domain is reduced (masked);
/// the function is total and never panics.
pub fn encode(id: u64, key: u64) -> u64 {
    let id = id & MAX_ID;
    feistel_n(id, key, ROUNDS, WIDTH_BITS)
}

/// Decodes a code in [0, MAX_ID] via inverse Feistel. Input outside the domain is reduced (masked);
/// the function is total and never panics.
pub fn decode(code: u64, key: u64) -> u64 {
    let code = code & MAX_ID;
    feistel_n_inv(code, key, ROUNDS, WIDTH_BITS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_sampled() {
        let key = 0x9E3779B97F4A7C15;
        for id in [0u64, 1, 2, 42, 1000, MAX_ID / 2, MAX_ID - 1, MAX_ID] {
            let code = encode(id, key);
            assert!(code <= MAX_ID, "code out of range: {code}");
            assert_eq!(decode(code, key), id, "round-trip failed for id={id}");
        }
    }

    #[test]
    fn bijectivity_on_small_width() {
        let key = 0xDEADBEEFCAFEBABE;
        let n = 1u64 << 20;
        let mut seen = vec![false; n as usize];
        for id in 0..n {
            let c = feistel_n(id, key, ROUNDS, 20);
            assert!(c < n);
            assert!(!seen[c as usize], "collision at id={id} -> {c}");
            seen[c as usize] = true;
        }
    }

    #[test]
    fn feistel_n_matches_encode_decode_anti_drift_guard() {
        let key = 0x1122334455667788;
        for id in [0u64, 1, 7, 12345, MAX_ID / 3, MAX_ID] {
            assert_eq!(encode(id, key), feistel_n(id, key, ROUNDS, WIDTH_BITS));
            assert_eq!(decode(id, key), feistel_n_inv(id, key, ROUNDS, WIDTH_BITS));
        }

        for &(rounds, width) in &[(4usize, 40u32), (6usize, 20u32)] {
            let x = (1u64 << (width / 2)) ^ 0x2A;
            let x = x & ((1u64 << width) - 1);
            let enc = feistel_n(x, key, rounds, width);
            assert_eq!(feistel_n_inv(enc, key, rounds, width), x);
        }
    }

    #[test]
    fn sequential_ids_are_not_enumerable() {
        let key = 0x0123456789ABCDEF;
        let a = encode(100, key);
        let b = encode(101, key);
        assert!(
            a.abs_diff(b) > 1,
            "neighboring codes are sequential: {a} {b}"
        );
    }
}
