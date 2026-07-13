// Espelha `codec::from_base62` + a checagem de colisão em `src/api.rs` do
// backend (Rust): um alias é rejeitado quando, decodificado como base62,
// ele cairia no mesmo espaço dos códigos numéricos gerados pelo sistema.
// Mantém o front alinhado ao 400 que a API devolveria de qualquer forma —
// aqui só evitamos o round-trip.
const ALPHABET = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const CODE_LEN = 7;
const MAX_ID = 2 ** 40 - 1; // 1_099_511_627_775 — WIDTH_BITS=40 em permute.rs

/**
 * `true` quando `s` é uma string base62 de exatamente 7 caracteres cujo
 * valor decodificado (big-endian, mesmo alfabeto do backend) cabe em 40
 * bits — ou seja, colide com o espaço de códigos numéricos e não pode ser
 * usada como alias.
 */
export function isNumericCode(s: string): boolean {
  if (s.length !== CODE_LEN) return false;
  let n = 0;
  for (const ch of s) {
    const digit = ALPHABET.indexOf(ch);
    if (digit === -1) return false;
    n = n * 62 + digit;
    if (n > MAX_ID) return false;
  }
  return true;
}

/** `true` quando `s`, ignorando espaços nas pontas, começa com http(s):// e é uma URL bem formada. */
export function isHttpUrl(s: string): boolean {
  try {
    const u = new URL(s.trim());
    return u.protocol === "http:" || u.protocol === "https:";
  } catch {
    return false;
  }
}
