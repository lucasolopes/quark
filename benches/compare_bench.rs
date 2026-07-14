//! Head-to-head: quark's ARX-Feistel short-code generation vs three real
//! competitor approaches, all pure-Rust crates, benched on the same machine
//! with the same criterion harness.
//!
//! Fairness notes (see also .superpowers/sdd/bench-compare-report.md):
//! - All benches operate on the SAME class of input: an integer id drawn
//!   from a wrapping counter over quark's domain (0..2^40), black-boxed on
//!   the way in and out so nothing is const-folded.
//! - quark: fixed 40-bit domain -> fixed 7-char output, keyed bijection
//!   (non-enumerable, zero collision checks by construction).
//! - sqids / harsh: arbitrary u64 domain, variable-length output,
//!   obfuscation (no cryptographic key / weak-or-known salt), not a keyed
//!   security primitive.
//! - feistel_hmac: same Feistel *structure* as quark (balanced, 4 rounds,
//!   40-bit width) but the round function is HMAC-SHA256 instead of ARX.
//!   This isolates quark's actual claim: ARX round vs cryptographic-hash
//!   round, holding the network structure constant.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use quark::codec;
use quark::permute::{self, MAX_ID};

type HmacSha256 = Hmac<Sha256>;

/// feistel_hmac: honest reproduction of the "real cipher" (Feistly-style)
/// approach. Same balanced-Feistel structure and round count as quark's
/// permute module, but the round function is HMAC-SHA256(key, round_byte
/// || R_bytes) truncated to 20 bits, instead of quark's ARX mix.
const FH_WIDTH_BITS: u32 = permute::WIDTH_BITS;
const FH_ROUNDS: usize = permute::ROUNDS;

fn fh_round_fn(key: &[u8], round: usize, half_bits: u32, r: u32) -> u32 {
    let mask: u32 = if half_bits >= 32 {
        u32::MAX
    } else {
        (1u32 << half_bits) - 1
    };
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(&[round as u8]);
    mac.update(&r.to_be_bytes());
    let tag = mac.finalize().into_bytes();
    let word = u32::from_be_bytes([tag[0], tag[1], tag[2], tag[3]]);
    word & mask
}

/// Encodes an id via the HMAC-round Feistel, then reuses quark's base62
/// step so the two approaches produce comparable opaque strings.
fn feistel_hmac_encode(id: u64, key: &[u8]) -> String {
    let half = FH_WIDTH_BITS / 2;
    let mask = (1u64 << half) - 1;
    let id = id & ((1u64 << FH_WIDTH_BITS) - 1);
    let mut l = ((id >> half) & mask) as u32;
    let mut r = (id & mask) as u32;
    for round in 0..FH_ROUNDS {
        let f = fh_round_fn(key, round, half, r);
        let new_l = r;
        let new_r = (l as u64 ^ f as u64) as u32 & (mask as u32);
        l = new_l;
        r = new_r;
    }
    let permuted = ((l as u64) << half) | (r as u64);
    codec::to_base62(permuted)
}

fn bench_quark(c: &mut Criterion) {
    let key = 0x9E3779B97F4A7C15;
    let mut group = c.benchmark_group("quark");

    group.bench_function("encode", |b| {
        let mut id = 0u64;
        b.iter(|| {
            id = id.wrapping_add(1) & MAX_ID;
            let code = permute::encode(black_box(id), key);
            black_box(codec::to_base62(black_box(code)))
        })
    });

    group.bench_function("decode", |b| {
        let mut id = 0u64;
        b.iter(|| {
            id = id.wrapping_add(1) & MAX_ID;
            let code = permute::encode(id, key);
            let s = codec::to_base62(code);
            let n = codec::from_base62(black_box(&s)).unwrap();
            black_box(permute::decode(black_box(n), key))
        })
    });

    group.finish();
}

fn bench_sqids(c: &mut Criterion) {
    let sqids = sqids::Sqids::new(None).expect("default sqids config is valid");
    let mut group = c.benchmark_group("sqids");

    group.bench_function("encode", |b| {
        let mut id = 0u64;
        b.iter(|| {
            id = id.wrapping_add(1) & MAX_ID;
            black_box(
                sqids
                    .encode(&[black_box(id)])
                    .expect("encode within default alphabet"),
            )
        })
    });

    group.bench_function("decode", |b| {
        let mut id = 0u64;
        b.iter(|| {
            id = id.wrapping_add(1) & MAX_ID;
            let s = sqids.encode(&[id]).unwrap();
            black_box(sqids.decode(black_box(&s)))
        })
    });

    group.finish();
}

fn bench_hashids(c: &mut Criterion) {
    let harsh = harsh::Harsh::default();
    let mut group = c.benchmark_group("hashids");

    group.bench_function("encode", |b| {
        let mut id = 0u64;
        b.iter(|| {
            id = id.wrapping_add(1) & MAX_ID;
            black_box(harsh.encode(&[black_box(id)]))
        })
    });

    group.bench_function("decode", |b| {
        let mut id = 0u64;
        b.iter(|| {
            id = id.wrapping_add(1) & MAX_ID;
            let s = harsh.encode(&[id]);
            black_box(
                harsh
                    .decode(black_box(&s))
                    .expect("valid hashid round-trips"),
            )
        })
    });

    group.finish();
}

fn bench_feistel_hmac(c: &mut Criterion) {
    let key = 0x9E3779B97F4A7C15u64.to_be_bytes();
    let mut group = c.benchmark_group("feistel_hmac");

    group.bench_function("encode", |b| {
        let mut id = 0u64;
        b.iter(|| {
            id = id.wrapping_add(1) & MAX_ID;
            black_box(feistel_hmac_encode(black_box(id), &key))
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_quark,
    bench_sqids,
    bench_hashids,
    bench_feistel_hmac
);
criterion_main!(benches);
