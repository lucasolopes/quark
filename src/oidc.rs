//! OIDC login (stage 1): configuration, discovery, PKCE, code exchange, and
//! id_token verification. Opt-in via `QUARK_OIDC_ISSUER`; the flow is driven by
//! the `/admin/login` and `/admin/callback` routes in `api.rs`. The panel admin
//! token stays a break-glass path regardless.

use crate::auth::Scope;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as b64url, Engine as _};
use hmac::{Hmac, KeyInit, Mac};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

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
    /// Where to send the browser after a successful login. Default `/` (panel
    /// same-origin); set to the panel URL for a split-origin deployment.
    pub post_login_url: String,
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
            post_login_url: std::env::var("QUARK_OIDC_POST_LOGIN_URL")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "/".to_string()),
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
/// Outcome of a failed id_token verification, distinguishing the one case a
/// JWKS refetch can fix (a signature that didn't verify, i.e. likely key
/// rotation) from definitive rejections (expiry, wrong issuer/audience,
/// azp/nonce/claims) where refetching would only hammer the IdP's jwks_uri.
pub enum VerifyError {
    /// Signature did not verify with this key; retry once with a fresh JWKS.
    BadSignature(String),
    /// Token is invalid for a non-key reason; do not refetch.
    Rejected(String),
}

impl VerifyError {
    pub fn message(&self) -> &str {
        match self {
            VerifyError::BadSignature(m) | VerifyError::Rejected(m) => m,
        }
    }
    fn retryable(&self) -> bool {
        matches!(self, VerifyError::BadSignature(_))
    }
}

pub fn verify_id_token(
    id_token: &str,
    key: &DecodingKey,
    issuer: &str,
    client_id: &str,
    nonce: &str,
) -> Result<Claims, VerifyError> {
    let mut val = Validation::new(Algorithm::RS256);
    val.set_issuer(&[issuer]);
    val.set_audience(&[client_id]);
    val.validate_exp = true;
    let data = decode::<serde_json::Value>(id_token, key, &val).map_err(|e| {
        // Only a signature mismatch is worth a JWKS refetch; expiry/issuer/
        // audience are definitive for this token regardless of the key set.
        match e.kind() {
            jsonwebtoken::errors::ErrorKind::InvalidSignature => {
                VerifyError::BadSignature(e.to_string())
            }
            _ => VerifyError::Rejected(e.to_string()),
        }
    })?;
    let claims = data.claims;
    // OIDC azp (authorized party): whenever it is present it MUST be our client
    // id, and a multi-audience token MUST carry it. This rejects a token minted
    // for a different client that merely lists us in `aud`.
    match claims.get("azp").and_then(|v| v.as_str()) {
        Some(azp) if azp != client_id => {
            return Err(VerifyError::Rejected("azp does not match client id".to_string()));
        }
        None => {
            if matches!(claims.get("aud"), Some(serde_json::Value::Array(a)) if a.len() > 1) {
                return Err(VerifyError::Rejected(
                    "multi-audience token without azp".to_string(),
                ));
            }
        }
        _ => {}
    }
    if claims.get("nonce").and_then(|v| v.as_str()) != Some(nonce) {
        return Err(VerifyError::Rejected("nonce mismatch".to_string()));
    }
    let subject = claims
        .get("sub")
        .and_then(|v| v.as_str())
        .ok_or_else(|| VerifyError::Rejected("id_token missing sub".to_string()))?
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

/// Live OIDC state held in `AppState`: the config, an HTTP client, the resolved
/// discovery document, and a refreshable JWKS.
pub struct OidcRuntime {
    pub config: OidcConfig,
    client: reqwest::Client,
    discovery: Discovery,
    jwks: tokio::sync::RwLock<Jwks>,
}

impl OidcRuntime {
    /// Resolves discovery and the initial JWKS for `config`.
    pub async fn init(config: OidcConfig) -> Result<OidcRuntime, String> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| e.to_string())?;
        let discovery = discover(&client, &config.issuer).await?;
        let jwks = fetch_jwks(&client, &discovery.jwks_uri).await?;
        Ok(OidcRuntime {
            config,
            client,
            discovery,
            jwks: tokio::sync::RwLock::new(jwks),
        })
    }

    /// The authorize URL for a fresh login attempt.
    pub fn authorize_url(&self, state: &str, nonce: &str, challenge: &str) -> String {
        authorize_url(&self.config, &self.discovery, state, nonce, challenge)
    }

    /// Exchanges a callback code for the id_token.
    pub async fn exchange_code(&self, code: &str, verifier: &str) -> Result<String, String> {
        exchange_code(&self.client, &self.config, &self.discovery, code, verifier).await
    }

    /// Verifies an id_token, refreshing the JWKS once only when it might help:
    /// the token's key id is absent from the cached set, or the signature did
    /// not verify (both signal IdP key rotation). Definitive rejections
    /// (expiry, issuer/audience, azp/nonce/claims) return immediately without a
    /// refetch, so a burst of bad logins can't hammer the provider's jwks_uri.
    pub async fn verify(&self, id_token: &str, nonce: &str) -> Result<Claims, String> {
        let kid = token_kid(id_token);
        {
            let jwks = self.jwks.read().await;
            if let Ok(key) = select_key(&jwks, kid.as_deref()) {
                match verify_id_token(
                    id_token,
                    &key,
                    &self.config.issuer,
                    &self.config.client_id,
                    nonce,
                ) {
                    Ok(claims) => return Ok(claims),
                    // Definitive: a fresh key set cannot change the outcome.
                    Err(e) if !e.retryable() => return Err(e.message().to_string()),
                    // Signature mismatch: fall through to refetch and retry.
                    Err(_) => {}
                }
            }
        }
        let fresh = fetch_jwks(&self.client, &self.discovery.jwks_uri).await?;
        let key = select_key(&fresh, kid.as_deref())?;
        let claims = verify_id_token(
            id_token,
            &key,
            &self.config.issuer,
            &self.config.client_id,
            nonce,
        )
        .map_err(|e| e.message().to_string())?;
        *self.jwks.write().await = fresh;
        Ok(claims)
    }
}

/// Signs the login-attempt state (`state.verifier.nonce`, all base64url so they
/// contain no `.`) with an HMAC, for the short-lived cookie that carries it from
/// `/admin/login` to `/admin/callback`. Value: `"state.verifier.nonce.mac"`.
pub fn sign_login_state(key: &[u8], state: &str, verifier: &str, nonce: &str) -> String {
    let payload = format!("{state}.{verifier}.{nonce}");
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(payload.as_bytes());
    format!("{payload}.{}", b64url.encode(mac.finalize().into_bytes()))
}

/// Verifies and unpacks a login-state cookie, returning `(state, verifier,
/// nonce)` when the HMAC checks out.
pub fn verify_login_state(key: &[u8], cookie: &str) -> Option<(String, String, String)> {
    let parts: Vec<&str> = cookie.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let (state, verifier, nonce, mac_b64) = (parts[0], parts[1], parts[2], parts[3]);
    let provided = b64url.decode(mac_b64).ok()?;
    let payload = format!("{state}.{verifier}.{nonce}");
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(payload.as_bytes());
    mac.verify_slice(&provided).ok()?;
    Some((state.to_string(), verifier.to_string(), nonce.to_string()))
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
            post_login_url: "/".into(),
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
    fn login_state_cookie_round_trip_and_tamper() {
        let key = b"login-state-signing-key-0123456789";
        let cookie = sign_login_state(key, "st8", "verif", "nnc");
        assert_eq!(
            verify_login_state(key, &cookie),
            Some(("st8".into(), "verif".into(), "nnc".into()))
        );
        // Wrong key rejected.
        assert!(verify_login_state(b"another-key-abcdefghijklmnopqrstuv", &cookie).is_none());
        // Tampered state rejected.
        let tampered = cookie.replacen("st8", "st9", 1);
        assert!(verify_login_state(key, &tampered).is_none());
        // Malformed rejected.
        assert!(verify_login_state(key, "garbage").is_none());
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
