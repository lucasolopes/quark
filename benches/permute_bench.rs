// Codigo de teste pode entrar em panico: a falha e o proprio sinal. O
// clippy.toml cobre itens sob #[test]/#[cfg(test)], mas nao os helpers de
// topo de arquivo (fn app(), fixtures), que sao a maioria aqui.
#![allow(clippy::unwrap_used)]

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use quark::permute::{decode, encode};

fn bench(c: &mut Criterion) {
    let key = 0x9E3779B97F4A7C15;
    c.bench_function("encode", |b| {
        let mut id = 0u64;
        b.iter(|| {
            id = id.wrapping_add(1) & quark::permute::MAX_ID;
            black_box(encode(black_box(id), key))
        })
    });
    c.bench_function("decode", |b| {
        let code = encode(12345, key);
        b.iter(|| black_box(decode(black_box(code), key)))
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
