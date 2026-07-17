//! Encryption at rest for stored third-party secrets — the per-tenant OIDC
//! `client_secret` and the Sheets `refresh_token` (LUC-48) — opt-in via
//! `QUARK_ENCRYPTION_KEY`. (Other stored values, e.g. webhook signing secrets,
//! are not covered by this module.)
//!
//! Sealed values are tagged `enc:v1:<base64(nonce || ciphertext)>`. Values
//! without that prefix are treated as legacy plaintext and passed through
//! unchanged by `open`, so existing unencrypted data keeps working when the
//! feature is turned on.

use base64::{engine::general_purpose::STANDARD as b64, Engine as _};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    XChaCha20Poly1305, XNonce,
};

/// Tag prefixed to every value sealed by this module.
const PREFIX: &str = "enc:v1:";

/// Nonce length for `XChaCha20Poly1305`, in bytes.
const NONCE_LEN: usize = 24;

/// Wraps an `XChaCha20Poly1305` cipher for sealing/opening secrets at rest.
pub struct SecretBox {
    cipher: XChaCha20Poly1305,
}

/// Failure opening a sealed value: malformed encoding, wrong nonce/ciphertext
/// split, or an authentication failure (wrong key or tampered data).
#[derive(Debug)]
pub enum SecretBoxError {
    /// The `enc:v1:` payload was not valid base64.
    InvalidEncoding,
    /// The decoded payload was shorter than one nonce, so there is no
    /// ciphertext to decrypt.
    Truncated,
    /// Decryption failed: wrong key or the ciphertext was tampered with.
    DecryptFailed,
}

impl std::fmt::Display for SecretBoxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecretBoxError::InvalidEncoding => write!(f, "sealed value is not valid base64"),
            SecretBoxError::Truncated => write!(f, "sealed value is shorter than one nonce"),
            SecretBoxError::DecryptFailed => write!(f, "decryption failed"),
        }
    }
}

impl std::error::Error for SecretBoxError {}

impl SecretBox {
    /// Builds a `SecretBox` from `QUARK_ENCRYPTION_KEY`: base64-decoded, must
    /// be exactly 32 bytes. Returns `None` (logging one line to stderr) when
    /// the env var is unset or does not decode to exactly 32 bytes — this is
    /// the opt-in switch for encryption at rest.
    pub fn from_env() -> Option<SecretBox> {
        let raw = std::env::var("QUARK_ENCRYPTION_KEY").ok()?;
        let bytes = match b64.decode(raw.trim()) {
            Ok(b) => b,
            Err(_) => {
                eprintln!("WARNING: QUARK_ENCRYPTION_KEY is not valid base64; secrets at rest will not be encrypted.");
                return None;
            }
        };
        let key: [u8; 32] = match bytes.try_into() {
            Ok(k) => k,
            Err(_) => {
                eprintln!("WARNING: QUARK_ENCRYPTION_KEY must decode to exactly 32 bytes; secrets at rest will not be encrypted.");
                return None;
            }
        };
        Some(SecretBox::from_key(key))
    }

    /// Builds a `SecretBox` from a raw 32-byte key. Exposed (not test-only)
    /// so callers that already have key material can construct one directly
    /// without round-tripping through an env var.
    pub fn from_key(key: [u8; 32]) -> SecretBox {
        SecretBox {
            cipher: XChaCha20Poly1305::new((&key).into()),
        }
    }

    /// Seals `plaintext`. An empty input returns an empty string unchanged —
    /// there is no secret to encrypt. Otherwise generates a fresh random
    /// 24-byte nonce and returns `enc:v1:base64(nonce || ciphertext)`.
    pub fn seal(&self, plaintext: &str) -> String {
        if plaintext.is_empty() {
            return String::new();
        }
        let mut nonce_bytes = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce_bytes).expect("system randomness source unavailable");
        let nonce = XNonce::from(nonce_bytes);
        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .expect("XChaCha20-Poly1305 encryption cannot fail for this input");
        let mut payload = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        payload.extend_from_slice(&nonce_bytes);
        payload.extend_from_slice(&ciphertext);
        format!("{PREFIX}{}", b64.encode(payload))
    }

    /// Opens `stored`. Values without the `enc:v1:` prefix are legacy
    /// plaintext and are returned unchanged. Prefixed values are
    /// base64-decoded, split into a 24-byte nonce and ciphertext, and
    /// decrypted; any format or authentication failure is an `Err`.
    pub fn open(&self, stored: &str) -> Result<String, SecretBoxError> {
        let Some(encoded) = stored.strip_prefix(PREFIX) else {
            return Ok(stored.to_string());
        };
        let payload = b64
            .decode(encoded)
            .map_err(|_| SecretBoxError::InvalidEncoding)?;
        if payload.len() < NONCE_LEN {
            return Err(SecretBoxError::Truncated);
        }
        let (nonce_bytes, ciphertext) = payload.split_at(NONCE_LEN);
        let nonce = XNonce::from_slice(nonce_bytes);
        let plaintext = self
            .cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| SecretBoxError::DecryptFailed)?;
        String::from_utf8(plaintext).map_err(|_| SecretBoxError::DecryptFailed)
    }
}

/// Seals `s` with `sb` when present, otherwise passes it through unchanged.
/// Lets call sites hold an `Option<SecretBox>` (encryption opt-in) without
/// branching on it at every use.
pub fn seal_opt(sb: &Option<SecretBox>, s: &str) -> String {
    match sb {
        Some(sb) => sb.seal(s),
        None => s.to_string(),
    }
}

/// Opens `s` with `sb` when present, otherwise passes it through unchanged.
pub fn open_opt(sb: &Option<SecretBox>, s: &str) -> Result<String, SecretBoxError> {
    match sb {
        Some(sb) => sb.open(s),
        None => Ok(s.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A: [u8; 32] = [1u8; 32];
    const KEY_B: [u8; 32] = [2u8; 32];

    #[test]
    fn seal_then_open_round_trips() {
        let sb = SecretBox::from_key(KEY_A);
        let sealed = sb.seal("top secret");
        assert_eq!(sb.open(&sealed).unwrap(), "top secret");
    }

    #[test]
    fn seal_uses_a_random_nonce_but_both_open_to_the_same_plaintext() {
        let sb = SecretBox::from_key(KEY_A);
        let a = sb.seal("same plaintext");
        let b = sb.seal("same plaintext");
        assert_ne!(a, b);
        assert_eq!(sb.open(&a).unwrap(), "same plaintext");
        assert_eq!(sb.open(&b).unwrap(), "same plaintext");
    }

    #[test]
    fn open_passes_through_legacy_plaintext_unchanged() {
        let sb = SecretBox::from_key(KEY_A);
        assert_eq!(sb.open("plain-secret").unwrap(), "plain-secret");
    }

    #[test]
    fn open_with_the_wrong_key_fails() {
        let sb_a = SecretBox::from_key(KEY_A);
        let sb_b = SecretBox::from_key(KEY_B);
        let sealed = sb_a.seal("top secret");
        assert!(sb_b.open(&sealed).is_err());
    }

    #[test]
    fn empty_string_seals_and_opens_to_empty() {
        let sb = SecretBox::from_key(KEY_A);
        assert_eq!(sb.seal(""), "");
        assert_eq!(sb.open("").unwrap(), "");
    }

    #[test]
    fn opt_helpers_pass_through_when_none() {
        let none: Option<SecretBox> = None;
        assert_eq!(seal_opt(&none, "x"), "x");
        assert_eq!(open_opt(&none, "x").unwrap(), "x");
    }

    #[test]
    fn opt_helpers_round_trip_when_some() {
        let some = Some(SecretBox::from_key(KEY_A));
        let sealed = seal_opt(&some, "x");
        assert_ne!(sealed, "x");
        assert_eq!(open_opt(&some, &sealed).unwrap(), "x");
    }

    #[test]
    fn open_of_a_too_short_base64_payload_is_truncated_error() {
        let sb = SecretBox::from_key(KEY_A);
        // 8 base64-encoded bytes, well short of the 24-byte nonce.
        let short = format!("{PREFIX}{}", b64.encode([0u8; 8]));
        assert!(matches!(sb.open(&short), Err(SecretBoxError::Truncated)));
    }

    #[test]
    fn open_of_non_base64_garbage_is_invalid_encoding_error() {
        let sb = SecretBox::from_key(KEY_A);
        let garbage = format!("{PREFIX}not-valid-base64!!!");
        assert!(matches!(
            sb.open(&garbage),
            Err(SecretBoxError::InvalidEncoding)
        ));
    }

    #[test]
    fn open_of_a_tampered_ciphertext_is_decrypt_failed_error() {
        let sb = SecretBox::from_key(KEY_A);
        let sealed = sb.seal("top secret");
        let encoded = sealed.strip_prefix(PREFIX).unwrap();
        let mut payload = b64.decode(encoded).unwrap();
        // Flip one byte past the nonce, inside the ciphertext, so the
        // AEAD authentication tag no longer matches.
        let last = payload.len() - 1;
        payload[last] ^= 0xFF;
        let tampered = format!("{PREFIX}{}", b64.encode(payload));
        assert!(matches!(
            sb.open(&tampered),
            Err(SecretBoxError::DecryptFailed)
        ));
    }
}
