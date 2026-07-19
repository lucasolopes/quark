//! Encryption at rest for stored third-party secrets — the per-tenant OIDC
//! `client_secret` and the Sheets `refresh_token` (LUC-48) — opt-in via
//! `QUARK_ENCRYPTION_KEY`. (Other stored values, e.g. webhook signing secrets,
//! are not covered by this module.)
//!
//! # Wire format
//!
//! - Plaintext (no prefix): legacy, passed through unchanged by `open`, so
//!   existing unencrypted data keeps working when the feature is turned on.
//! - `enc:v1:<base64(nonce || ciphertext)>`: the original LUC-48 format. No
//!   AAD, single key. Still opened for back-compat: every key in the keyring is
//!   tried with an empty AAD.
//! - `enc:v2:<keyid>:<base64(nonce || ciphertext)>`: LUC-62. `keyid` selects
//!   the key from the keyring, and the AEAD is bound to a caller-supplied `aad`
//!   (the row identity, `tenant:field`). Copying a v2 ciphertext to another
//!   row/tenant no longer decrypts, because the AAD no longer matches.
//!
//! `keyid = hex(SHA-256(key)[..4])` (8 hex chars): deterministic from the key
//! material, so no extra operator config is needed to name a key.
//!
//! # Keyring and rotation
//!
//! `SecretBox` is a keyring: one primary key (`QUARK_ENCRYPTION_KEY`, used for
//! every new seal) plus zero or more decrypt-only old keys
//! (`QUARK_ENCRYPTION_KEY_OLD`, comma-separated) kept around so values sealed
//! under a previous key still open during a rotation. The boot backfill then
//! re-seals everything under the primary key.

use base64::{engine::general_purpose::STANDARD as b64, Engine as _};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    XChaCha20Poly1305, XNonce,
};
use sha2::{Digest, Sha256};

/// Tag prefixed to every value sealed in the original LUC-48 format.
const PREFIX_V1: &str = "enc:v1:";

/// Tag prefixed to every value sealed by this module (LUC-62). The full form is
/// `enc:v2:<keyid>:<base64(nonce || ciphertext)>`.
const PREFIX_V2: &str = "enc:v2:";

/// Nonce length for `XChaCha20Poly1305`, in bytes.
const NONCE_LEN: usize = 24;

/// One key in the keyring: its `keyid` (see module docs) and the cipher built
/// from it.
struct KeyEntry {
    id: String,
    cipher: XChaCha20Poly1305,
}

/// A keyring for sealing/opening secrets at rest. `keys[primary]` is the key
/// every new seal uses; the rest are decrypt-only, kept for rotation.
pub struct SecretBox {
    keys: Vec<KeyEntry>,
    primary: usize,
}

/// Failure opening a sealed value: malformed encoding, wrong nonce/ciphertext
/// split, an unknown key id, or an authentication failure (wrong key, wrong
/// AAD, or tampered data). No variant carries any plaintext.
#[derive(Debug)]
pub enum SecretBoxError {
    /// The payload (or the `enc:v2:` structure) was not well-formed.
    InvalidEncoding,
    /// The decoded payload was shorter than one nonce, so there is no
    /// ciphertext to decrypt.
    Truncated,
    /// The `enc:v2:` key id does not match any key in the keyring.
    UnknownKey,
    /// Decryption failed: wrong key, wrong AAD, or the ciphertext was tampered
    /// with.
    DecryptFailed,
}

impl std::fmt::Display for SecretBoxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecretBoxError::InvalidEncoding => write!(f, "sealed value is malformed"),
            SecretBoxError::Truncated => write!(f, "sealed value is shorter than one nonce"),
            SecretBoxError::UnknownKey => write!(f, "sealed value uses an unknown key id"),
            SecretBoxError::DecryptFailed => write!(f, "decryption failed"),
        }
    }
}

impl std::error::Error for SecretBoxError {}

/// Computes the key id: hex of the first 4 bytes of `SHA-256(key)`.
/// Deterministic from the key, so the same key always names the same id.
fn keyid(key: &[u8; 32]) -> String {
    let digest = Sha256::digest(key);
    let mut out = String::with_capacity(8);
    for b in &digest[..4] {
        out.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        out.push(char::from_digit((b & 0x0f) as u32, 16).unwrap());
    }
    out
}

impl SecretBox {
    /// Builds a `SecretBox` from the environment: the primary key from
    /// `QUARK_ENCRYPTION_KEY` and any decrypt-only old keys from
    /// `QUARK_ENCRYPTION_KEY_OLD` (comma-separated). Every value is
    /// base64-decoded and must be exactly 32 bytes.
    ///
    /// Returns `None` (logging one line to stderr) when `QUARK_ENCRYPTION_KEY`
    /// is unset or does not decode to exactly 32 bytes — this is the opt-in
    /// switch for encryption at rest. An invalid old key logs and is skipped
    /// (rotation degrades to "some old values will not open" rather than
    /// turning encryption off).
    pub fn from_env() -> Option<SecretBox> {
        let raw = std::env::var("QUARK_ENCRYPTION_KEY").ok()?;
        let primary = match decode_key(&raw) {
            Some(k) => k,
            None => {
                eprintln!("WARNING: QUARK_ENCRYPTION_KEY is not valid base64 for exactly 32 bytes; secrets at rest will not be encrypted.");
                return None;
            }
        };

        let mut olds: Vec<[u8; 32]> = Vec::new();
        if let Ok(raw_olds) = std::env::var("QUARK_ENCRYPTION_KEY_OLD") {
            for part in raw_olds.split(',') {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                match decode_key(part) {
                    Some(k) => olds.push(k),
                    None => eprintln!(
                        "WARNING: an entry in QUARK_ENCRYPTION_KEY_OLD is not valid base64 for exactly 32 bytes; that old key is ignored."
                    ),
                }
            }
        }

        Some(SecretBox::from_keys(primary, &olds))
    }

    /// Builds a single-key `SecretBox` from a raw 32-byte key. Exposed (not
    /// test-only) so callers that already have key material can construct one
    /// directly without round-tripping through an env var.
    pub fn from_key(key: [u8; 32]) -> SecretBox {
        SecretBox::from_keys(key, &[])
    }

    /// Builds a keyring with `primary` as the sealing key plus `olds` as
    /// decrypt-only keys (for rotation). The primary is always at index 0.
    pub fn from_keys(primary: [u8; 32], olds: &[[u8; 32]]) -> SecretBox {
        let mut keys = Vec::with_capacity(1 + olds.len());
        keys.push(KeyEntry {
            id: keyid(&primary),
            cipher: XChaCha20Poly1305::new((&primary).into()),
        });
        for k in olds {
            keys.push(KeyEntry {
                id: keyid(k),
                cipher: XChaCha20Poly1305::new(k.into()),
            });
        }
        SecretBox { keys, primary: 0 }
    }

    /// The key id of the primary (sealing) key. Used by the backfill to decide
    /// whether a stored value is already sealed under the current key.
    pub fn primary_keyid(&self) -> &str {
        &self.keys[self.primary].id
    }

    /// Seals `plaintext`, binding the ciphertext to `aad` (the row identity).
    /// An empty input returns an empty string unchanged — there is no secret to
    /// encrypt. Otherwise generates a fresh random 24-byte nonce and returns
    /// `enc:v2:<keyid>:base64(nonce || ciphertext)` using the primary key.
    pub fn seal(&self, plaintext: &str, aad: &[u8]) -> String {
        if plaintext.is_empty() {
            return String::new();
        }
        let entry = &self.keys[self.primary];
        let mut nonce_bytes = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce_bytes).expect("system randomness source unavailable");
        let nonce = XNonce::from(nonce_bytes);
        let ciphertext = entry
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext.as_bytes(),
                    aad,
                },
            )
            .expect("XChaCha20-Poly1305 encryption cannot fail for this input");
        let mut payload = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        payload.extend_from_slice(&nonce_bytes);
        payload.extend_from_slice(&ciphertext);
        format!("{PREFIX_V2}{}:{}", entry.id, b64.encode(payload))
    }

    /// Opens `stored`, verifying `aad` for `enc:v2:` values.
    ///
    /// Decision tree:
    /// - No known prefix: legacy plaintext, returned unchanged (`aad` ignored).
    /// - `enc:v1:`: original format, no AAD. Every key in the keyring is tried
    ///   with an empty AAD; the first success wins. `aad` is ignored (a v1
    ///   value never carried one). No downgrade the other way: a v2 value is
    ///   never opened as v1.
    /// - `enc:v2:<keyid>:`: the key is selected by `keyid` (unknown id →
    ///   `UnknownKey`, no brute force over other keys) and decryption is bound
    ///   to `aad`. A wrong AAD authenticates as a `DecryptFailed`.
    pub fn open(&self, stored: &str, aad: &[u8]) -> Result<String, SecretBoxError> {
        if let Some(rest) = stored.strip_prefix(PREFIX_V2) {
            return self.open_v2(rest, aad);
        }
        if let Some(encoded) = stored.strip_prefix(PREFIX_V1) {
            return self.open_v1(encoded);
        }
        // No known prefix: legacy plaintext.
        Ok(stored.to_string())
    }

    /// Opens an `enc:v1:` body (everything after the prefix): try every key
    /// with an empty AAD, first success wins.
    fn open_v1(&self, encoded: &str) -> Result<String, SecretBoxError> {
        let payload = b64
            .decode(encoded)
            .map_err(|_| SecretBoxError::InvalidEncoding)?;
        if payload.len() < NONCE_LEN {
            return Err(SecretBoxError::Truncated);
        }
        let (nonce_bytes, ciphertext) = payload.split_at(NONCE_LEN);
        let nonce = XNonce::from_slice(nonce_bytes);
        for entry in &self.keys {
            if let Ok(pt) = entry.cipher.decrypt(
                nonce,
                Payload {
                    msg: ciphertext,
                    aad: b"",
                },
            ) {
                return String::from_utf8(pt).map_err(|_| SecretBoxError::DecryptFailed);
            }
        }
        Err(SecretBoxError::DecryptFailed)
    }

    /// Opens an `enc:v2:` body (everything after the `enc:v2:` prefix, i.e.
    /// `<keyid>:<base64>`): select the key by id, decrypt binding `aad`.
    fn open_v2(&self, rest: &str, aad: &[u8]) -> Result<String, SecretBoxError> {
        let (key_id, encoded) = rest
            .split_once(':')
            .ok_or(SecretBoxError::InvalidEncoding)?;
        if key_id.is_empty() {
            return Err(SecretBoxError::InvalidEncoding);
        }
        let entry = self
            .keys
            .iter()
            .find(|e| e.id == key_id)
            .ok_or(SecretBoxError::UnknownKey)?;
        let payload = b64
            .decode(encoded)
            .map_err(|_| SecretBoxError::InvalidEncoding)?;
        if payload.len() < NONCE_LEN {
            return Err(SecretBoxError::Truncated);
        }
        let (nonce_bytes, ciphertext) = payload.split_at(NONCE_LEN);
        let nonce = XNonce::from_slice(nonce_bytes);
        let plaintext = entry
            .cipher
            .decrypt(
                nonce,
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .map_err(|_| SecretBoxError::DecryptFailed)?;
        String::from_utf8(plaintext).map_err(|_| SecretBoxError::DecryptFailed)
    }
}

/// Decodes a base64 key that must be exactly 32 bytes; `None` otherwise.
fn decode_key(raw: &str) -> Option<[u8; 32]> {
    let bytes = b64.decode(raw.trim()).ok()?;
    bytes.try_into().ok()
}

/// Seals `s` with `sb` when present, otherwise passes it through unchanged.
/// Lets call sites hold an `Option<SecretBox>` (encryption opt-in) without
/// branching on it at every use. When encryption is off, `aad` is ignored.
pub fn seal_opt(sb: &Option<SecretBox>, s: &str, aad: &[u8]) -> String {
    match sb {
        Some(sb) => sb.seal(s, aad),
        None => s.to_string(),
    }
}

/// Opens `s` with `sb` when present, otherwise passes it through unchanged.
/// When encryption is off, `aad` is ignored.
pub fn open_opt(sb: &Option<SecretBox>, s: &str, aad: &[u8]) -> Result<String, SecretBoxError> {
    match sb {
        Some(sb) => sb.open(s, aad),
        None => Ok(s.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A: [u8; 32] = [1u8; 32];
    const KEY_B: [u8; 32] = [2u8; 32];
    const KEY_C: [u8; 32] = [3u8; 32];

    const AAD: &[u8] = b"7:oidc_client_secret";
    const OTHER_AAD: &[u8] = b"8:oidc_client_secret";

    #[test]
    fn seal_then_open_round_trips_with_aad() {
        let sb = SecretBox::from_key(KEY_A);
        let sealed = sb.seal("top secret", AAD);
        assert!(sealed.starts_with("enc:v2:"));
        assert_eq!(sb.open(&sealed, AAD).unwrap(), "top secret");
    }

    #[test]
    fn open_v2_with_a_different_aad_is_decrypt_failed() {
        // The per-row bind actually works: the same ciphertext copied to a row
        // with a different (tenant:field) AAD does not decrypt.
        let sb = SecretBox::from_key(KEY_A);
        let sealed = sb.seal("top secret", AAD);
        assert!(matches!(
            sb.open(&sealed, OTHER_AAD),
            Err(SecretBoxError::DecryptFailed)
        ));
    }

    #[test]
    fn open_v2_with_a_keyid_not_in_the_keyring_is_unknown_key() {
        let sb_a = SecretBox::from_key(KEY_A);
        let sealed = sb_a.seal("top secret", AAD);
        // A keyring that has neither KEY_A nor its id.
        let sb_b = SecretBox::from_key(KEY_B);
        assert!(matches!(
            sb_b.open(&sealed, AAD),
            Err(SecretBoxError::UnknownKey)
        ));
    }

    #[test]
    fn rotation_old_key_opens_and_new_seal_uses_primary() {
        // A value sealed under KEY_B (old), opened by a keyring whose primary
        // is KEY_A and whose old key is KEY_B.
        let sb_b = SecretBox::from_key(KEY_B);
        let sealed_old = sb_b.seal("rotate me", AAD);

        let sb = SecretBox::from_keys(KEY_A, &[KEY_B]);
        assert_eq!(sb.open(&sealed_old, AAD).unwrap(), "rotate me");

        // A fresh seal uses the primary (KEY_A) key id, not KEY_B's.
        let resealed = sb.seal("rotate me", AAD);
        let prefix = format!("enc:v2:{}:", keyid(&KEY_A));
        assert!(
            resealed.starts_with(&prefix),
            "expected reseal under primary keyid {}, got {resealed}",
            keyid(&KEY_A)
        );
        assert!(!resealed.starts_with(&format!("enc:v2:{}:", keyid(&KEY_B))));
    }

    #[test]
    fn v1_value_still_opens_with_the_keyring_ignoring_aad() {
        // Build a genuine v1 value by hand (the current code only writes v2).
        let sb = SecretBox::from_key(KEY_A);
        let mut nonce_bytes = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce_bytes).unwrap();
        let nonce = XNonce::from(nonce_bytes);
        let ct = sb.keys[0]
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: b"legacy v1",
                    aad: b"",
                },
            )
            .unwrap();
        let mut payload = nonce_bytes.to_vec();
        payload.extend_from_slice(&ct);
        let v1 = format!("{PREFIX_V1}{}", b64.encode(payload));

        // Opens even though a non-empty AAD is passed (v1 never carried one).
        assert_eq!(sb.open(&v1, AAD).unwrap(), "legacy v1");
        // And opens through a rotated keyring too (tries every key, empty AAD).
        let sb_rot = SecretBox::from_keys(KEY_C, &[KEY_A]);
        assert_eq!(sb_rot.open(&v1, AAD).unwrap(), "legacy v1");
    }

    #[test]
    fn seal_uses_a_random_nonce_but_both_open_to_the_same_plaintext() {
        let sb = SecretBox::from_key(KEY_A);
        let a = sb.seal("same plaintext", AAD);
        let b = sb.seal("same plaintext", AAD);
        assert_ne!(a, b);
        assert_eq!(sb.open(&a, AAD).unwrap(), "same plaintext");
        assert_eq!(sb.open(&b, AAD).unwrap(), "same plaintext");
    }

    #[test]
    fn open_passes_through_legacy_plaintext_unchanged() {
        let sb = SecretBox::from_key(KEY_A);
        assert_eq!(sb.open("plain-secret", AAD).unwrap(), "plain-secret");
    }

    #[test]
    fn open_with_the_wrong_key_fails() {
        let sb_a = SecretBox::from_key(KEY_A);
        let sb_b = SecretBox::from_key(KEY_B);
        let sealed = sb_a.seal("top secret", AAD);
        // KEY_B's keyring does not know KEY_A's id at all.
        assert!(sb_b.open(&sealed, AAD).is_err());
    }

    #[test]
    fn empty_string_seals_and_opens_to_empty() {
        let sb = SecretBox::from_key(KEY_A);
        assert_eq!(sb.seal("", AAD), "");
        assert_eq!(sb.open("", AAD).unwrap(), "");
    }

    #[test]
    fn keyid_is_stable_for_the_same_key() {
        assert_eq!(keyid(&KEY_A), keyid(&KEY_A));
        assert_ne!(keyid(&KEY_A), keyid(&KEY_B));
        assert_eq!(keyid(&KEY_A).len(), 8);
    }

    #[test]
    fn opt_helpers_pass_through_when_none() {
        let none: Option<SecretBox> = None;
        assert_eq!(seal_opt(&none, "x", AAD), "x");
        assert_eq!(open_opt(&none, "x", AAD).unwrap(), "x");
    }

    #[test]
    fn opt_helpers_round_trip_when_some() {
        let some = Some(SecretBox::from_key(KEY_A));
        let sealed = seal_opt(&some, "x", AAD);
        assert_ne!(sealed, "x");
        assert_eq!(open_opt(&some, &sealed, AAD).unwrap(), "x");
    }

    #[test]
    fn open_of_a_too_short_base64_payload_is_truncated_error() {
        let sb = SecretBox::from_key(KEY_A);
        // 8 base64-encoded bytes, well short of the 24-byte nonce, under a v2
        // envelope with the primary key id.
        let short = format!("enc:v2:{}:{}", keyid(&KEY_A), b64.encode([0u8; 8]));
        assert!(matches!(
            sb.open(&short, AAD),
            Err(SecretBoxError::Truncated)
        ));
    }

    #[test]
    fn open_of_non_base64_garbage_is_invalid_encoding_error() {
        let sb = SecretBox::from_key(KEY_A);
        let garbage = format!("enc:v2:{}:not-valid-base64!!!", keyid(&KEY_A));
        assert!(matches!(
            sb.open(&garbage, AAD),
            Err(SecretBoxError::InvalidEncoding)
        ));
    }

    #[test]
    fn open_of_v2_without_a_keyid_separator_is_invalid_encoding() {
        let sb = SecretBox::from_key(KEY_A);
        // No second colon after the version tag: cannot tell key id from body.
        let malformed = format!("enc:v2:{}", b64.encode([0u8; NONCE_LEN + 16]));
        assert!(matches!(
            sb.open(&malformed, AAD),
            Err(SecretBoxError::InvalidEncoding)
        ));
    }

    #[test]
    fn open_of_a_tampered_ciphertext_is_decrypt_failed_error() {
        let sb = SecretBox::from_key(KEY_A);
        let sealed = sb.seal("top secret", AAD);
        // Strip `enc:v2:<keyid>:` to reach the base64 body.
        let rest = sealed.strip_prefix(PREFIX_V2).unwrap();
        let (kid, encoded) = rest.split_once(':').unwrap();
        let mut payload = b64.decode(encoded).unwrap();
        // Flip one byte past the nonce, inside the ciphertext, so the AEAD
        // authentication tag no longer matches.
        let last = payload.len() - 1;
        payload[last] ^= 0xFF;
        let tampered = format!("enc:v2:{kid}:{}", b64.encode(payload));
        assert!(matches!(
            sb.open(&tampered, AAD),
            Err(SecretBoxError::DecryptFailed)
        ));
    }
}
