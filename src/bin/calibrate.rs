//! Harness de avalanche/SAC para calibrar o número de rounds da permutação.
//! Porte espiritual do diffusion_sac.c do lab de SHA-256.
//! Roda offline: `cargo run --bin calibrate`.

use quark::permute::feistel_n;

const WIDTH: u32 = 40;
const SAMPLES: u64 = 200_000;

fn main() {
    let key = 0x9E3779B97F4A7C15u64;
    println!("rounds | avalanche_medio | bits_saida_cobertos(/{WIDTH})");
    for rounds in 1..=12usize {
        let mut total_flips: u64 = 0;
        // matriz de dependência: dependency[i][j] = bit de saída j já mudou ao virar bit de entrada i?
        let mut dep = vec![0u64; WIDTH as usize]; // bitmask de saída por bit de entrada
        // gerador pseudo-aleatório simples (LCG) — determinístico, sem depender de Date/rand.
        let mut seed = 0xCAFEF00DD15EA5E5u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..SAMPLES {
            let x = next() & MASK40;
            let base = feistel_n(x, key, rounds, WIDTH);
            for i in 0..WIDTH {
                let y = feistel_n(x ^ (1u64 << i), key, rounds, WIDTH);
                let diff = base ^ y;
                total_flips += diff.count_ones() as u64;
                dep[i as usize] |= diff;
            }
        }
        let avg = total_flips as f64 / (SAMPLES as f64 * WIDTH as f64 * WIDTH as f64);
        // cobertura: menor número de bits de saída afetados por algum bit de entrada
        let cobertos = dep.iter().map(|m| m.count_ones()).min().unwrap_or(0);
        println!("{rounds:6} | {avg:.4}          | {cobertos}");
    }
    println!("\nCritério: escolha o menor `rounds` com avalanche ~0.50 e cobertura = {WIDTH}.");
}

const MASK40: u64 = (1u64 << 40) - 1;
