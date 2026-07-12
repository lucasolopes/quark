//! Bijeção sobre [0, 2^WIDTH_BITS) via rede de Feistel balanceada
//! com função de round ARX. É format-preserving: encode/decode nunca
//! saem do range, nunca colidem.

pub const WIDTH_BITS: u32 = 40;
pub const ROUNDS: usize = 4; // calibrado (ver bin/calibrate): menor nº de rounds com avalanche = 0,5000 exata e cobertura 40/40 (difusão fechada); r3 encosta em 0,4866
pub const MAX_ID: u64 = (1u64 << WIDTH_BITS) - 1;

/// Deriva a subchave do round a partir da chave mestra e do índice.
#[inline]
fn subkey(key: u64, round: usize) -> u32 {
    // mistura simples chave+round; espalha os bits altos da chave.
    let x = key
        .rotate_left((round as u32) * 7 + 1)
        ^ (0x9E3779B97F4A7C15u64.wrapping_mul(round as u64 + 1));
    (x ^ (x >> 32)) as u32
}

/// Função de round ARX: mistura um meio (half_bits) com a subchave.
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

/// Feistel genérico sobre `width` bits (width par), parameterizado pelo nº de
/// rounds. Generaliza o Feistel usado por encode; também usado pelo teste de
/// bijetividade em largura pequena e pelo harness de calibração (bin/calibrate),
/// que mede a permutação real (sem cópia divergente).
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
        // inverte um round: antes tínhamos (l,r) = (r_prev, l_prev ^ f(r_prev))
        let r_prev = l;
        let f = round_fn(r_prev, key, round, half);
        let l_prev = r ^ f;
        l = l_prev;
        r = r_prev;
    }
    ((l as u64) << half) | (r as u64)
}

/// Codifica um id em [0, MAX_ID] via Feistel. Entrada fora do domínio é reduzida (mascarada);
/// a função é total e nunca dá panic.
pub fn encode(id: u64, key: u64) -> u64 {
    let id = id & MAX_ID;
    feistel_n(id, key, ROUNDS, WIDTH_BITS)
}

/// Decodifica um code em [0, MAX_ID] via Feistel inversa. Entrada fora do domínio é reduzida (mascarada);
/// a função é total e nunca dá panic.
pub fn decode(code: u64, key: u64) -> u64 {
    let code = code & MAX_ID;
    feistel_n_inv(code, key, ROUNDS, WIDTH_BITS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_amostrado() {
        let key = 0x9E3779B97F4A7C15;
        for id in [0u64, 1, 2, 42, 1000, MAX_ID / 2, MAX_ID - 1, MAX_ID] {
            let code = encode(id, key);
            assert!(code <= MAX_ID, "code fora do range: {code}");
            assert_eq!(decode(code, key), id, "round-trip falhou para id={id}");
        }
    }

    #[test]
    fn bijetividade_em_largura_pequena() {
        // Varre um domínio pequeno inteiro e prova que encode é permutação.
        // Usa 20 bits mascarando; a estrutura Feistel é a mesma.
        let key = 0xDEADBEEFCAFEBABE;
        let n = 1u64 << 20;
        let mut visto = vec![false; n as usize];
        for id in 0..n {
            let c = feistel_n(id, key, ROUNDS, 20);
            assert!(c < n);
            assert!(!visto[c as usize], "colisão em id={id} -> {c}");
            visto[c as usize] = true;
        }
    }

    #[test]
    fn feistel_n_bate_com_encode_decode_guarda_anti_drift() {
        // Pin de comportamento: encode/decode devem ser exatamente feistel_n/feistel_n_inv
        // com ROUNDS/WIDTH_BITS do módulo. Se um futuro refactor desviar a função de round
        // usada pelo calibrate, este teste (ou o round-trip) quebra primeiro.
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
    fn nao_enumeravel_ids_vizinhos() {
        // ids sequenciais não produzem códigos sequenciais.
        let key = 0x0123456789ABCDEF;
        let a = encode(100, key);
        let b = encode(101, key);
        assert!(a.abs_diff(b) > 1, "códigos vizinhos são sequenciais: {a} {b}");
    }
}
