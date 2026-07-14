/**
 * Mirrors `codec::from_base62` + the collision check in the backend's
 * `src/api.rs` (Rust): an alias is rejected when, decoded as base62, it
 * would fall in the same space as the system-generated numeric codes. Keeps
 * the frontend aligned with the 400 the API would return anyway — here we
 * just avoid the round-trip.
 */
const ALPHABET = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const CODE_LEN = 7;
const MAX_ID = 2 ** 40 - 1;

/**
 * `true` when `s` is a base62 string of exactly 7 characters whose decoded
 * value (big-endian, same alphabet as the backend) fits in 40 bits — i.e. it
 * collides with the numeric-code space and cannot be used as an alias.
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

/**
 * `true` when `s`, ignoring leading/trailing whitespace, starts with
 * `http://` or `https://` — the same check the backend performs
 * (`starts_with`, `src/api.rs`): a case-sensitive prefix comparison, no
 * parsing via `URL`. A scheme like `HTTP://` would pass JS's `new URL` but
 * would be rejected by the backend; matching this here avoids that mismatch.
 */
export function isHttpUrl(s: string): boolean {
  const trimmed = s.trim();
  return trimmed.startsWith("http://") || trimmed.startsWith("https://");
}
