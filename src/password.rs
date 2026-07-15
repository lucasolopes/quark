//! Per-link password hashing.
//!
//! Link passwords are hashed with argon2id and stored as a PHC string in
//! `Record.password_hash`. Verification runs only on the unlock POST (which is
//! rate-limited), never on the redirect hot path, so the deliberate slowness of
//! argon2 never touches redirect latency. The salt comes from the system RNG via
//! `getrandom` (the same source the rest of the crate uses), so we do not pull in
//! the `rand` crate just for a salt.

use argon2::password_hash::{Error, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as b64, Engine as _};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// How long a successful unlock is remembered by the signed cookie.
pub const UNLOCK_TTL_SECS: u64 = 12 * 3600;

/// HMAC-SHA256 over `"<code>.<expiry>.<password_hash>"` keyed by the dedicated
/// 32-byte signing key, base64url (no pad). Binding the current password hash
/// means rotating (or removing) the password changes the hash and instantly
/// invalidates every outstanding unlock token; binding the code and expiry means
/// a token cannot be replayed for another link or after it lapses.
fn sign_unlock(key: &[u8], code: &str, expiry: u64, password_hash: &str) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(code.as_bytes());
    mac.update(b".");
    mac.update(expiry.to_string().as_bytes());
    mac.update(b".");
    mac.update(password_hash.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// Builds the unlock cookie *value* (`"<expiry>.<base64url(mac)>"`) for a code
/// unlocked at `now`, plus the absolute `expiry` (for the cookie's `Max-Age`).
/// The caller adds the cookie name/attributes. `password_hash` is the link's
/// current hash, bound into the token so it dies when the password changes.
pub fn unlock_token(key: &[u8], code: &str, password_hash: &str, now: u64) -> (String, u64) {
    let expiry = now.saturating_add(UNLOCK_TTL_SECS);
    let mac = b64.encode(sign_unlock(key, code, expiry, password_hash));
    (format!("{expiry}.{mac}"), expiry)
}

/// Whether an unlock cookie value is a valid, unexpired token for `code` under
/// the link's current `password_hash`. Constant-time MAC comparison via
/// `verify_slice`.
pub fn unlock_token_valid(
    token: &str,
    key: &[u8],
    code: &str,
    password_hash: &str,
    now: u64,
) -> bool {
    let Some((exp_str, mac_b64)) = token.split_once('.') else {
        return false;
    };
    let Ok(expiry) = exp_str.parse::<u64>() else {
        return false;
    };
    if expiry <= now {
        return false;
    }
    let Ok(provided) = b64.decode(mac_b64) else {
        return false;
    };
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(code.as_bytes());
    mac.update(b".");
    mac.update(expiry.to_string().as_bytes());
    mac.update(b".");
    mac.update(password_hash.as_bytes());
    mac.verify_slice(&provided).is_ok()
}

/// Hashes a plaintext link password into an argon2id PHC string suitable for
/// storing in `Record.password_hash`. The plaintext is never persisted.
pub fn hash_password(plaintext: &str) -> Result<String, Error> {
    let mut salt_bytes = [0u8; 16];
    getrandom::fill(&mut salt_bytes).expect("system RNG must be available");
    let salt = SaltString::encode_b64(&salt_bytes)?;
    let hash = Argon2::default().hash_password(plaintext.as_bytes(), &salt)?;
    Ok(hash.to_string())
}

/// Verifies a submitted password against a stored argon2 PHC string. Returns
/// `false` (never panics) on any parse or verification failure, so a malformed
/// stored hash denies access rather than crashing the redirect path. The
/// comparison is constant-time (argon2 verifies the whole hash).
pub fn verify_password(plaintext: &str, phc: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(phc) else {
        return false;
    };
    Argon2::default()
        .verify_password(plaintext.as_bytes(), &parsed)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::{hash_password, verify_password};

    #[test]
    fn hash_then_verify_true() {
        let phc = hash_password("s3cret-pass").unwrap();
        assert!(verify_password("s3cret-pass", &phc));
    }

    #[test]
    fn wrong_password_verifies_false() {
        let phc = hash_password("s3cret-pass").unwrap();
        assert!(!verify_password("wrong", &phc));
    }

    #[test]
    fn malformed_hash_verifies_false_without_panicking() {
        assert!(!verify_password("anything", "not-a-phc-string"));
        assert!(!verify_password("anything", ""));
    }

    #[test]
    fn two_hashes_of_same_password_differ_by_salt() {
        let a = hash_password("same").unwrap();
        let b = hash_password("same").unwrap();
        assert_ne!(a, b, "distinct salts must yield distinct PHC strings");
        assert!(verify_password("same", &a) && verify_password("same", &b));
    }

    use super::{unlock_token, unlock_token_valid};

    const KEY: &[u8; 32] = b"unit-test-signing-key-0123456789";
    const KEY2: &[u8; 32] = b"another-signing-key-abcdefghijkl";
    const H: &str = "$argon2id$v=19$m=19456,t=2,p=1$aaaa$bbbb";
    const H2: &str = "$argon2id$v=19$m=19456,t=2,p=1$cccc$dddd";

    #[test]
    fn fresh_unlock_token_is_valid() {
        let (tok, _exp) = unlock_token(KEY, "abc123", H, 1000);
        assert!(unlock_token_valid(&tok, KEY, "abc123", H, 1000));
    }

    #[test]
    fn unlock_token_for_another_code_is_rejected() {
        let (tok, _) = unlock_token(KEY, "abc123", H, 1000);
        assert!(!unlock_token_valid(&tok, KEY, "other", H, 1000));
    }

    #[test]
    fn unlock_token_with_wrong_key_is_rejected() {
        let (tok, _) = unlock_token(KEY, "abc123", H, 1000);
        assert!(!unlock_token_valid(&tok, KEY2, "abc123", H, 1000));
    }

    #[test]
    fn unlock_token_after_password_rotation_is_rejected() {
        // Rotating the password changes the hash, which must invalidate the token.
        let (tok, _) = unlock_token(KEY, "abc123", H, 1000);
        assert!(!unlock_token_valid(&tok, KEY, "abc123", H2, 1000));
    }

    #[test]
    fn expired_unlock_token_is_rejected() {
        let (tok, exp) = unlock_token(KEY, "abc123", H, 1000);
        assert!(!unlock_token_valid(&tok, KEY, "abc123", H, exp));
        assert!(!unlock_token_valid(&tok, KEY, "abc123", H, exp + 1));
    }

    #[test]
    fn tampered_unlock_token_is_rejected() {
        let (tok, _) = unlock_token(KEY, "abc123", H, 1000);
        assert!(!unlock_token_valid("garbage", KEY, "abc123", H, 1000));
        assert!(!unlock_token_valid(&format!("{tok}x"), KEY, "abc123", H, 1000));
    }
}
