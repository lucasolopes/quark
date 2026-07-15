//! OIDC login (stage 1): configuration, discovery, PKCE, code exchange, and
//! id_token verification. Opt-in via `QUARK_OIDC_ISSUER`; the flow is driven by
//! the `/admin/login` and `/admin/callback` routes in `api.rs`. The panel admin
//! token stays a break-glass path regardless.

use crate::auth::Scope;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as b64url, Engine as _};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// OIDC settings read once from the environment. `from_env` returns `None` when
/// `QUARK_OIDC_ISSUER` is unset, which keeps OIDC fully off by default.
#[derive(Debug, Clone)]
pub struct OidcConfig {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: String,
    /// Space-separated scopes requested (default `openid profile email`).
    pub scopes: String,
    /// Claim inspected for authorization (e.g. `groups`), and the values that
    /// grant each role. Default-closed: no match grants nothing.
    pub admin_claim: String,
    pub admin_value: String,
    pub readonly_value: Option<String>,
}

impl OidcConfig {
    pub fn from_env() -> Option<OidcConfig> {
        let issuer = std::env::var("QUARK_OIDC_ISSUER").ok().filter(|s| !s.is_empty())?;
        Some(OidcConfig {
            issuer: issuer.trim_end_matches('/').to_string(),
            client_id: std::env::var("QUARK_OIDC_CLIENT_ID").unwrap_or_default(),
            client_secret: std::env::var("QUARK_OIDC_CLIENT_SECRET").unwrap_or_default(),
            redirect_url: std::env::var("QUARK_OIDC_REDIRECT_URL").unwrap_or_default(),
            scopes: std::env::var("QUARK_OIDC_SCOPES")
                .unwrap_or_else(|_| "openid profile email".to_string()),
            admin_claim: std::env::var("QUARK_OIDC_ADMIN_CLAIM").unwrap_or_else(|_| "groups".into()),
            admin_value: std::env::var("QUARK_OIDC_ADMIN_VALUE").unwrap_or_default(),
            readonly_value: std::env::var("QUARK_OIDC_READONLY_VALUE").ok().filter(|s| !s.is_empty()),
        })
    }
}

/// The subset of the IdP's `.well-known/openid-configuration` we use.
#[derive(Debug, Clone, Deserialize)]
pub struct Discovery {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
}

/// Fetches the IdP discovery document.
pub async fn discover(client: &reqwest::Client, issuer: &str) -> Result<Discovery, String> {
    let url = format!("{}/.well-known/openid-configuration", issuer.trim_end_matches('/'));
    let resp = client.get(&url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("discovery HTTP {}", resp.status()));
    }
    resp.json::<Discovery>().await.map_err(|e| e.to_string())
}

/// A single RSA JWK (the only key type quark verifies).
#[derive(Debug, Clone, Deserialize)]
pub struct Jwk {
    pub kid: Option<String>,
    pub n: String,
    pub e: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Jwks {
    pub keys: Vec<Jwk>,
}

/// Fetches the IdP JWKS (RSA signing keys).
pub async fn fetch_jwks(client: &reqwest::Client, jwks_uri: &str) -> Result<Jwks, String> {
    let resp = client.get(jwks_uri).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("jwks HTTP {}", resp.status()));
    }
    resp.json::<Jwks>().await.map_err(|e| e.to_string())
}

/// Reads the `kid` (key id) from a JWT header, so the caller can pick the right
/// JWKS key before verifying.
pub fn token_kid(id_token: &str) -> Option<String> {
    decode_header(id_token).ok().and_then(|h| h.kid)
}

/// Builds a verification key for the JWT's `kid` from a JWKS. When the token
/// carries no `kid` and there is exactly one key, that key is used.
pub fn select_key(jwks: &Jwks, kid: Option<&str>) -> Result<DecodingKey, String> {
    let jwk = match kid {
        Some(kid) => jwks.keys.iter().find(|k| k.kid.as_deref() == Some(kid)),
        None if jwks.keys.len() == 1 => jwks.keys.first(),
        None => None,
    }
    .ok_or_else(|| "no matching JWK for token kid".to_string())?;
    DecodingKey::from_rsa_components(&jwk.n, &jwk.e).map_err(|e| e.to_string())
}

/// A random PKCE verifier and its S256 challenge (base64url, no pad).
pub fn pkce_pair() -> (String, String) {
    let mut raw = [0u8; 32];
    getrandom::fill(&mut raw).expect("system RNG must be available");
    let verifier = b64url.encode(raw);
    let challenge = b64url.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

/// A random opaque value (state / nonce), base64url.
pub fn random_token() -> String {
    let mut raw = [0u8; 24];
    getrandom::fill(&mut raw).expect("system RNG must be available");
    b64url.encode(raw)
}

/// Builds the IdP authorize URL for the Authorization Code + PKCE flow.
pub fn authorize_url(
    cfg: &OidcConfig,
    disco: &Discovery,
    state: &str,
    nonce: &str,
    challenge: &str,
) -> String {
    let q = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("response_type", "code")
        .append_pair("client_id", &cfg.client_id)
        .append_pair("redirect_uri", &cfg.redirect_url)
        .append_pair("scope", &cfg.scopes)
        .append_pair("state", state)
        .append_pair("nonce", nonce)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .finish();
    let sep = if disco.authorization_endpoint.contains('?') { '&' } else { '?' };
    format!("{}{}{}", disco.authorization_endpoint, sep, q)
}

/// Exchanges an authorization `code` (with the PKCE `verifier`) for the token
/// response and returns the raw `id_token`.
pub async fn exchange_code(
    client: &reqwest::Client,
    cfg: &OidcConfig,
    disco: &Discovery,
    code: &str,
    verifier: &str,
) -> Result<String, String> {
    #[derive(Deserialize)]
    struct TokenResp {
        id_token: Option<String>,
    }
    let params = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", cfg.redirect_url.as_str()),
        ("client_id", cfg.client_id.as_str()),
        ("client_secret", cfg.client_secret.as_str()),
        ("code_verifier", verifier),
    ];
    let resp = client
        .post(&disco.token_endpoint)
        .form(&params)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("token endpoint HTTP {}", resp.status()));
    }
    let body = resp.json::<TokenResp>().await.map_err(|e| e.to_string())?;
    body.id_token.ok_or_else(|| "token response missing id_token".to_string())
}

/// The claims quark reads out of a verified id_token.
#[derive(Debug, Clone)]
pub struct Claims {
    pub subject: String,
    pub display: String,
    pub raw: serde_json::Value,
}

/// Verifies an id_token against `key`: RS256 signature, issuer, audience
/// (client id), expiry (via the JWT `exp`), and the `nonce` bound at login.
/// Returns the extracted claims on success.
pub fn verify_id_token(
    id_token: &str,
    key: &DecodingKey,
    issuer: &str,
    client_id: &str,
    nonce: &str,
) -> Result<Claims, String> {
    let mut val = Validation::new(Algorithm::RS256);
    val.set_issuer(&[issuer]);
    val.set_audience(&[client_id]);
    val.validate_exp = true;
    let data = decode::<serde_json::Value>(id_token, key, &val).map_err(|e| e.to_string())?;
    let claims = data.claims;
    if claims.get("nonce").and_then(|v| v.as_str()) != Some(nonce) {
        return Err("nonce mismatch".to_string());
    }
    let subject = claims
        .get("sub")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "id_token missing sub".to_string())?
        .to_string();
    let display = claims
        .get("email")
        .and_then(|v| v.as_str())
        .or_else(|| claims.get("name").and_then(|v| v.as_str()))
        .or_else(|| claims.get("preferred_username").and_then(|v| v.as_str()))
        .unwrap_or(&subject)
        .to_string();
    Ok(Claims { subject, display, raw: claims })
}

/// Maps verified claims to granted scopes, default-closed: only the configured
/// admin value grants `Full`; the optional read-only value grants
/// `LinksRead`+`Analytics`; anything else grants nothing.
pub fn map_scopes(claims: &serde_json::Value, cfg: &OidcConfig) -> Vec<Scope> {
    let claim = claims.get(&cfg.admin_claim);
    let has = |needle: &str| -> bool {
        match claim {
            Some(serde_json::Value::String(s)) => s == needle,
            Some(serde_json::Value::Array(arr)) => {
                arr.iter().any(|v| v.as_str() == Some(needle))
            }
            _ => false,
        }
    };
    if !cfg.admin_value.is_empty() && has(&cfg.admin_value) {
        return vec![Scope::Full];
    }
    if let Some(ro) = &cfg.readonly_value {
        if has(ro) {
            return vec![Scope::LinksRead, Scope::Analytics];
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> OidcConfig {
        OidcConfig {
            issuer: "https://idp.example".into(),
            client_id: "quark".into(),
            client_secret: "secret".into(),
            redirect_url: "https://q.example/admin/callback".into(),
            scopes: "openid profile email".into(),
            admin_claim: "groups".into(),
            admin_value: "quark-admins".into(),
            readonly_value: Some("quark-viewers".into()),
        }
    }

    #[test]
    fn pkce_challenge_is_s256_of_verifier() {
        let (verifier, challenge) = pkce_pair();
        let expected = b64url.encode(Sha256::digest(verifier.as_bytes()));
        assert_eq!(challenge, expected);
        assert_ne!(verifier, challenge);
    }

    #[test]
    fn authorize_url_has_required_params() {
        let disco = Discovery {
            authorization_endpoint: "https://idp.example/authorize".into(),
            token_endpoint: "https://idp.example/token".into(),
            jwks_uri: "https://idp.example/jwks".into(),
        };
        let u = authorize_url(&cfg(), &disco, "st8", "nnc", "chlng");
        for needle in [
            "response_type=code",
            "client_id=quark",
            "code_challenge=chlng",
            "code_challenge_method=S256",
            "state=st8",
            "nonce=nnc",
            "scope=openid+profile+email",
        ] {
            assert!(u.contains(needle), "missing {needle} in {u}");
        }
        assert!(u.starts_with("https://idp.example/authorize?"));
    }

    #[test]
    fn map_scopes_is_default_closed() {
        let c = cfg();
        // admin group -> Full
        let admin = serde_json::json!({ "groups": ["x", "quark-admins"] });
        assert_eq!(map_scopes(&admin, &c), vec![Scope::Full]);
        // read-only group -> read scopes
        let ro = serde_json::json!({ "groups": ["quark-viewers"] });
        assert_eq!(map_scopes(&ro, &c), vec![Scope::LinksRead, Scope::Analytics]);
        // string claim form
        let admin_str = serde_json::json!({ "groups": "quark-admins" });
        assert_eq!(map_scopes(&admin_str, &c), vec![Scope::Full]);
        // no matching group -> nothing
        let none = serde_json::json!({ "groups": ["random"] });
        assert!(map_scopes(&none, &c).is_empty());
        // missing claim -> nothing
        let missing = serde_json::json!({ "sub": "x" });
        assert!(map_scopes(&missing, &c).is_empty());
    }
}
