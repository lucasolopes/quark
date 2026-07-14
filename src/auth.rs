use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Permission a token can be granted. `Full` is the superuser scope: it covers
/// every other scope, matching the always-`Full` behavior of the env
/// `QUARK_ADMIN_TOKEN`. Every other variant only covers itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    LinksRead,
    LinksWrite,
    Blocklist,
    Webhooks,
    Analytics,
    Full,
}

impl Scope {
    /// Whether this scope satisfies a `required` scope. `Full` satisfies
    /// anything; every other scope only satisfies itself.
    pub fn covers(&self, required: Scope) -> bool {
        *self == Scope::Full || *self == required
    }
}

/// A named API token: the plaintext is generated once by `generate_token`
/// and never persisted, only its SHA-256 hash (`token_hash`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiToken {
    pub id: u64,
    pub name: String,
    pub token_hash: String,
    pub scopes: Vec<Scope>,
    pub rate_limit_per_min: Option<u32>,
    pub created: u64,
}

/// Base62 alphabet (digits, uppercase, lowercase) used by `generate_token`.
const BASE62_ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Number of random base62 characters after the `qtok_` prefix.
const TOKEN_BODY_LEN: usize = 32;

/// Generates a new plaintext API token: `qtok_` followed by
/// `TOKEN_BODY_LEN` cryptographically random base62 characters.
pub fn generate_token() -> String {
    let mut raw = [0u8; TOKEN_BODY_LEN];
    getrandom::fill(&mut raw).expect("system randomness source unavailable");
    let mut token = String::with_capacity("qtok_".len() + TOKEN_BODY_LEN);
    token.push_str("qtok_");
    for b in raw {
        token.push(BASE62_ALPHABET[(b as usize) % BASE62_ALPHABET.len()] as char);
    }
    token
}

/// Deterministic SHA-256 hex digest of a token, used as the persisted
/// lookup key (the plaintext itself is never stored).
pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_token_is_deterministic() {
        let a = hash_token("qtok_abc123");
        let b = hash_token("qtok_abc123");
        assert_eq!(a, b);
    }

    #[test]
    fn hash_token_is_hex_sha256() {
        let h = hash_token("qtok_abc123");
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn full_scope_covers_every_scope() {
        for required in [
            Scope::LinksRead,
            Scope::LinksWrite,
            Scope::Blocklist,
            Scope::Webhooks,
            Scope::Analytics,
            Scope::Full,
        ] {
            assert!(Scope::Full.covers(required));
        }
    }

    #[test]
    fn non_full_scope_only_covers_itself() {
        assert!(Scope::LinksRead.covers(Scope::LinksRead));
        assert!(!Scope::LinksRead.covers(Scope::LinksWrite));
        assert!(!Scope::LinksWrite.covers(Scope::LinksRead));
        assert!(!Scope::Blocklist.covers(Scope::Full));
    }

    #[test]
    fn scope_serde_rename_is_lowercase_snake_case() {
        assert_eq!(
            serde_json::to_string(&Scope::LinksRead).unwrap(),
            "\"links_read\""
        );
        assert_eq!(
            serde_json::to_string(&Scope::LinksWrite).unwrap(),
            "\"links_write\""
        );
        assert_eq!(
            serde_json::to_string(&Scope::Blocklist).unwrap(),
            "\"blocklist\""
        );
        assert_eq!(
            serde_json::to_string(&Scope::Webhooks).unwrap(),
            "\"webhooks\""
        );
        assert_eq!(
            serde_json::to_string(&Scope::Analytics).unwrap(),
            "\"analytics\""
        );
        assert_eq!(serde_json::to_string(&Scope::Full).unwrap(), "\"full\"");
    }

    #[test]
    fn generate_token_has_expected_prefix_and_length() {
        let t = generate_token();
        assert!(t.starts_with("qtok_"));
        assert!(t.len() >= "qtok_".len() + 32);
        assert!(t["qtok_".len()..]
            .chars()
            .all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn generate_token_is_random() {
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b);
    }

    #[test]
    fn api_token_round_trips_through_json() {
        let tok = ApiToken {
            id: 1,
            name: "ci".into(),
            token_hash: hash_token("qtok_abc123"),
            scopes: vec![Scope::LinksRead, Scope::Webhooks],
            rate_limit_per_min: Some(60),
            created: 100,
        };
        let json = serde_json::to_string(&tok).unwrap();
        let back: ApiToken = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, 1);
        assert_eq!(back.name, "ci");
        assert_eq!(back.scopes, vec![Scope::LinksRead, Scope::Webhooks]);
        assert_eq!(back.rate_limit_per_min, Some(60));
    }
}
