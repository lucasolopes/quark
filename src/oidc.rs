//! OIDC login (stage 1): configuration, discovery, PKCE, code exchange, and
//! id_token verification. Opt-in via `QUARK_OIDC_ISSUER`; the flow is driven by
//! the `/admin/login` and `/admin/callback` routes in `api.rs`. The panel admin
//! token stays a break-glass path regardless.

use crate::auth::Scope;
use crate::store::{Store, StoreError};
use crate::tenant::{Membership, Role, TenantId, User, DEFAULT_TENANT};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as b64url, Engine as _};
use hmac::{Hmac, KeyInit, Mac};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

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
        let issuer = std::env::var("QUARK_OIDC_ISSUER")
            .ok()
            .filter(|s| !s.is_empty())?;
        Some(OidcConfig {
            issuer: issuer.trim_end_matches('/').to_string(),
            client_id: std::env::var("QUARK_OIDC_CLIENT_ID").unwrap_or_default(),
            client_secret: std::env::var("QUARK_OIDC_CLIENT_SECRET").unwrap_or_default(),
            redirect_url: std::env::var("QUARK_OIDC_REDIRECT_URL").unwrap_or_default(),
            scopes: std::env::var("QUARK_OIDC_SCOPES")
                .unwrap_or_else(|_| "openid profile email".to_string()),
            admin_claim: std::env::var("QUARK_OIDC_ADMIN_CLAIM")
                .unwrap_or_else(|_| "groups".into()),
            admin_value: std::env::var("QUARK_OIDC_ADMIN_VALUE").unwrap_or_default(),
            readonly_value: std::env::var("QUARK_OIDC_READONLY_VALUE")
                .ok()
                .filter(|s| !s.is_empty()),
            post_login_url: std::env::var("QUARK_OIDC_POST_LOGIN_URL")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "/".to_string()),
        })
    }
}

/// A tenant's own OIDC IdP (multi-tenancy P2d, cloud-only). One per tenant
/// (`oidc_configs.tenant_id` is UNIQUE); `issuer` is a plain column, the rest
/// rides in the `blob` (see `Store::put_oidc_config`/`get_oidc_config`).
/// `client_secret` is encrypted at rest when `QUARK_ENCRYPTION_KEY` is set
/// (LUC-48, opt-in via `secretbox`); unset, it is stored plaintext.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantOidcConfig {
    pub tenant_id: TenantId,
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    /// Space-separated-at-request-time scopes, kept as a list here.
    pub scopes: Vec<String>,
    pub admin_claim: String,
    pub admin_value: String,
    pub readonly_value: String,
    /// Optional required-group gate (multi-tenancy P2d Task 4b), default-open
    /// when unset: `claim_role`'s open Member default (any authenticated
    /// tenant IdP user gets in) is unchanged. When set to a non-empty value,
    /// `passes_required_group` denies anyone whose claim contains neither
    /// `admin_value`, `readonly_value`, nor this value — the tenant opts into
    /// default-closed login. `#[serde(default)]` so a blob written before
    /// this field existed still deserializes.
    #[serde(default)]
    pub required_value: Option<String>,
    pub post_login_url: Option<String>,
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
    let url = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    );
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
    let resp = client
        .get(jwks_uri)
        .send()
        .await
        .map_err(|e| e.to_string())?;
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
    let sep = if disco.authorization_endpoint.contains('?') {
        '&'
    } else {
        '?'
    };
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
    body.id_token
        .ok_or_else(|| "token response missing id_token".to_string())
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
            return Err(VerifyError::Rejected(
                "azp does not match client id".to_string(),
            ));
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
    Ok(Claims {
        subject,
        display,
        raw: claims,
    })
}

/// Whether `claims[claim_name]` contains `needle`, as either a single string
/// claim or an array claim (the two shapes IdPs commonly use for group/role
/// claims). Shared by `map_scopes` and `claim_role`.
fn claim_contains(claims: &serde_json::Value, claim_name: &str, needle: &str) -> bool {
    match claims.get(claim_name) {
        Some(serde_json::Value::String(s)) => s == needle,
        Some(serde_json::Value::Array(arr)) => arr.iter().any(|v| v.as_str() == Some(needle)),
        _ => false,
    }
}

/// Maps verified claims to granted scopes, default-closed: only the configured
/// admin value grants `Full`; the optional read-only value grants
/// `LinksRead`+`Analytics`; anything else grants nothing.
pub fn map_scopes(claims: &serde_json::Value, cfg: &OidcConfig) -> Vec<Scope> {
    if !cfg.admin_value.is_empty() && claim_contains(claims, &cfg.admin_claim, &cfg.admin_value) {
        return vec![Scope::Full];
    }
    if let Some(ro) = &cfg.readonly_value {
        if claim_contains(claims, &cfg.admin_claim, ro) {
            return vec![Scope::LinksRead, Scope::Analytics];
        }
    }
    Vec::new()
}

/// Maps a tenant IdP's group claim to a `Membership` role (multi-tenancy P2d,
/// per-tenant login). Mirrors `map_scopes`'s claim shape handling, but targets
/// a `Role` rather than a `Scope`: `admin_value` present in the claim grants
/// `Role::Admin`, `readonly_value` grants `Role::Viewer`, and anything else
/// (including a claim that matches neither) grants the default `Role::Member`
/// — every authenticated tenant IdP user gets at least member access, never
/// none, unlike the default-closed OSS/global `map_scopes`. `Role::Owner` is
/// never returned here: Owner comes only from creating the tenant, never from
/// an IdP claim.
pub fn claim_role(claims: &serde_json::Value, cfg: &TenantOidcConfig) -> Role {
    if !cfg.admin_value.is_empty() && claim_contains(claims, &cfg.admin_claim, &cfg.admin_value) {
        return Role::Admin;
    }
    if !cfg.readonly_value.is_empty()
        && claim_contains(claims, &cfg.admin_claim, &cfg.readonly_value)
    {
        return Role::Viewer;
    }
    Role::Member
}

/// The required-group gate (multi-tenancy P2d Task 4b), separate from
/// `claim_role`: `claim_role` always resolves to *some* role (Admin, Viewer,
/// or the open Member default), but whether that login is admitted at all is
/// this function's call. When `cfg.required_value` is unset (or empty), the
/// gate is open — every authenticated tenant IdP user is admitted, matching
/// today's behavior before this field existed. When set, only a user whose
/// claim contains `admin_value`, `readonly_value`, or `required_value` is
/// admitted; a claim matching none of the three is denied. Matching goes
/// through `claim_contains` (exact value match, not substring), the same
/// helper `claim_role`/`map_scopes` use. The caller (`oidc_callback`) must
/// check this BEFORE creating any membership or session — a denial here
/// grants nothing.
pub fn passes_required_group(claims: &serde_json::Value, cfg: &TenantOidcConfig) -> bool {
    let Some(required) = cfg.required_value.as_deref().filter(|r| !r.is_empty()) else {
        return true;
    };
    (!cfg.admin_value.is_empty() && claim_contains(claims, &cfg.admin_claim, &cfg.admin_value))
        || (!cfg.readonly_value.is_empty()
            && claim_contains(claims, &cfg.admin_claim, &cfg.readonly_value))
        || claim_contains(claims, &cfg.admin_claim, required)
}

/// Resolves the `User` for a verified login (creating one on first login, keyed
/// by the immutable `subject`). Returns the user id the caller should bind the
/// new `Session` to.
///
/// OSS (`multi_tenant == false`): also upserts the `Membership` in
/// `DEFAULT_TENANT`, with a `role` aligned to the same IdP group that produced
/// `scopes`. `tenant_membership` is ignored in this mode (the caller never
/// passes one for the OSS/global login path).
///
/// Cloud (`multi_tenant == true`): the global env-IdP login (`tenant_membership
/// == None`) creates no membership — a cloud user starts with 0 memberships
/// until they create or are invited to a workspace (P2b/P2c). A per-tenant
/// login (multi-tenancy P2d, `?org=<slug>`) passes `Some((tenant, role))`,
/// where `role` came from `claim_role` against that tenant's own IdP config;
/// this upserts a `Membership(user, tenant, role)` so signing in through the
/// tenant's own IdP is itself how a member joins that tenant.
///
/// Authorization is unaffected by this: for OSS it is decided by `scopes`
/// (from `map_scopes`); for cloud it is decided by the membership role at
/// request time (`admin_guard`); the stored `role` here is what grants that
/// authorization for the tenant path, not merely a record.
pub async fn ensure_user_and_membership(
    store: &dyn Store,
    multi_tenant: bool,
    subject: &str,
    email: &str,
    display: &str,
    scopes: &[Scope],
    tenant_membership: Option<(TenantId, Role)>,
) -> Result<u64, StoreError> {
    let user = match store.get_user_by_subject(subject).await? {
        Some(u) => u,
        None => {
            let id = store.next_user_id().await?;
            let u = User {
                id,
                subject: subject.to_string(),
                email: email.to_string(),
                display: display.to_string(),
                created: crate::now(),
            };
            store.put_user(&u).await?;
            u
        }
    };
    if !multi_tenant {
        // OSS: single implicit tenant 0. Cloud: no membership until the user
        // creates or is invited to a workspace (P2b/P2c), unless this is a
        // per-tenant login (see `tenant_membership` below).
        let role = if scopes.contains(&Scope::Full) {
            Role::Admin
        } else {
            Role::Viewer
        };
        store
            .put_membership(&Membership {
                user_id: user.id,
                tenant_id: DEFAULT_TENANT,
                role,
                created: crate::now(),
            })
            .await?;
    } else if let Some((tenant, role)) = tenant_membership {
        // Never let a login claim downgrade an existing Owner. `claim_role`
        // can't produce `Role::Owner` on its own, so the only way a user has
        // it is a prior explicit grant (workspace creation, invite accept) —
        // preserve it rather than overwrite it with whatever the IdP group
        // maps to today. Any other existing role (or none yet) still follows
        // the claim, so group changes keep reflecting for non-owners.
        let existing = store.get_membership(user.id, tenant).await?;
        let effective_role = match existing {
            Some(m) if m.role == Role::Owner => Role::Owner,
            _ => role,
        };
        store
            .put_membership(&Membership {
                user_id: user.id,
                tenant_id: tenant,
                role: effective_role,
                created: crate::now(),
            })
            .await?;
    }
    Ok(user.id)
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
        Self::build(config).await
    }

    /// Builds a runtime from a tenant's stored OIDC config (multi-tenancy
    /// P2d): same discovery + JWKS init as `init`, from a `TenantOidcConfig`
    /// instead of the env-sourced `OidcConfig`. The redirect URL is not part
    /// of the stored per-tenant config — every tenant's IdP redirects to the
    /// same `/admin/callback` route, which resolves the tenant from the
    /// signed login-state cookie rather than from a per-tenant redirect
    /// URI — so it still comes from `QUARK_OIDC_REDIRECT_URL`.
    pub async fn from_config(cfg: &TenantOidcConfig) -> Result<OidcRuntime, String> {
        let config = OidcConfig {
            issuer: cfg.issuer.trim_end_matches('/').to_string(),
            client_id: cfg.client_id.clone(),
            client_secret: cfg.client_secret.clone(),
            redirect_url: std::env::var("QUARK_OIDC_REDIRECT_URL").unwrap_or_default(),
            scopes: if cfg.scopes.is_empty() {
                "openid profile email".to_string()
            } else {
                cfg.scopes.join(" ")
            },
            admin_claim: cfg.admin_claim.clone(),
            admin_value: cfg.admin_value.clone(),
            readonly_value: (!cfg.readonly_value.is_empty()).then(|| cfg.readonly_value.clone()),
            post_login_url: cfg
                .post_login_url
                .clone()
                .unwrap_or_else(|| "/".to_string()),
        };
        Self::build(config).await
    }

    async fn build(config: OidcConfig) -> Result<OidcRuntime, String> {
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

/// Per-tenant `OidcRuntime` cache (multi-tenancy P2d): each tenant's own IdP
/// gets its own discovery + JWKS, built lazily on first use (`get_or_build`)
/// and cached for `TTL_SECS` so a reconfigured IdP is picked up within a
/// bounded window even if the explicit `invalidate` call on `PUT`/`DELETE
/// /admin/oidc-config` is missed (invalidation is best-effort by design — a
/// miss just means the next login re-fetches the current stored config).
pub struct TenantOidcCache {
    cache: moka::future::Cache<TenantId, Arc<OidcRuntime>>,
}

/// How long a built runtime is trusted before a rebuild is forced, bounding
/// how stale a tenant's cached IdP config (issuer, JWKS, claim mapping) can
/// get after a reconfiguration that missed the explicit invalidation.
const TENANT_OIDC_TTL_SECS: u64 = 300;

impl TenantOidcCache {
    pub fn new() -> TenantOidcCache {
        TenantOidcCache {
            cache: moka::future::Cache::builder()
                .time_to_live(std::time::Duration::from_secs(TENANT_OIDC_TTL_SECS))
                .build(),
        }
    }

    /// Returns the cached runtime for `tenant`, building (discovery + JWKS,
    /// via `OidcRuntime::from_config`) and caching it on a miss.
    pub async fn get_or_build(
        &self,
        tenant: TenantId,
        cfg: &TenantOidcConfig,
    ) -> Result<Arc<OidcRuntime>, String> {
        if let Some(rt) = self.cache.get(&tenant).await {
            return Ok(rt);
        }
        let rt = Arc::new(OidcRuntime::from_config(cfg).await?);
        self.cache.insert(tenant, rt.clone()).await;
        Ok(rt)
    }

    /// Drops the cached runtime for `tenant`. Called (best-effort) by the
    /// `PUT`/`DELETE /admin/oidc-config` handlers so a reconfigured or
    /// removed IdP isn't served from a stale cache entry; a miss here is
    /// harmless since the entry also expires via the TTL.
    pub async fn invalidate(&self, tenant: TenantId) {
        self.cache.invalidate(&tenant).await;
    }
}

impl Default for TenantOidcCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Signs the login-attempt state (`state.verifier.nonce`, all base64url so they
/// contain no `.`) with an HMAC, for the short-lived cookie that carries it from
/// `/admin/login` to `/admin/callback`. `tenant` is `Some` for a per-tenant
/// login (`?org=<slug>`, multi-tenancy P2d) and `None` for the global/OSS
/// login; when present it rides in the HMAC-signed payload as a 4th field, so
/// `verify_login_state` can trust it came from this login attempt and was not
/// substituted in transit. Value: `"state.verifier.nonce.tenant.mac"` (tenant
/// empty when absent, still covered by the MAC), back-compat with the old
/// 3-field callers via `None`.
pub fn sign_login_state(
    key: &[u8],
    state: &str,
    verifier: &str,
    nonce: &str,
    tenant: Option<TenantId>,
) -> String {
    let tenant_field = tenant.map(|t| t.0.to_string()).unwrap_or_default();
    let payload = format!("{state}.{verifier}.{nonce}.{tenant_field}");
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(payload.as_bytes());
    format!("{payload}.{}", b64url.encode(mac.finalize().into_bytes()))
}

/// Verifies and unpacks a login-state cookie, returning `(state, verifier,
/// nonce, tenant)` when the HMAC checks out. `tenant` is `None` when the login
/// was global (no `?org`), `Some` when it was a per-tenant login — recomputed
/// from the same payload the MAC was computed over, so a tampered tenant field
/// fails verification rather than being silently accepted.
pub fn verify_login_state(
    key: &[u8],
    cookie: &str,
) -> Option<(String, String, String, Option<TenantId>)> {
    let parts: Vec<&str> = cookie.split('.').collect();
    if parts.len() != 5 {
        return None;
    }
    let (state, verifier, nonce, tenant_field, mac_b64) =
        (parts[0], parts[1], parts[2], parts[3], parts[4]);
    let provided = b64url.decode(mac_b64).ok()?;
    let payload = format!("{state}.{verifier}.{nonce}.{tenant_field}");
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(payload.as_bytes());
    mac.verify_slice(&provided).ok()?;
    let tenant = if tenant_field.is_empty() {
        None
    } else {
        Some(TenantId(tenant_field.parse::<u64>().ok()?))
    };
    Some((
        state.to_string(),
        verifier.to_string(),
        nonce.to_string(),
        tenant,
    ))
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
        let cookie = sign_login_state(key, "st8", "verif", "nnc", None);
        assert_eq!(
            verify_login_state(key, &cookie),
            Some(("st8".into(), "verif".into(), "nnc".into(), None))
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
    fn login_state_cookie_carries_tenant_and_tamper_is_rejected() {
        let key = b"login-state-signing-key-0123456789";
        let tenant = TenantId(42);
        let cookie = sign_login_state(key, "st8", "verif", "nnc", Some(tenant));
        assert_eq!(
            verify_login_state(key, &cookie),
            Some(("st8".into(), "verif".into(), "nnc".into(), Some(tenant)))
        );

        // Absent tenant (global login) round-trips as None.
        let global_cookie = sign_login_state(key, "st8", "verif", "nnc", None);
        assert_eq!(
            verify_login_state(key, &global_cookie).unwrap().3,
            None,
            "absent tenant must mean global login"
        );

        // Tampering the tenant field (swap 42 for 43) must fail the MAC, not
        // silently authenticate as a different tenant.
        let tampered = cookie.replacen(".42.", ".43.", 1);
        assert_ne!(tampered, cookie);
        assert!(
            verify_login_state(key, &tampered).is_none(),
            "tampered tenant field must be rejected, not accepted as tenant 43"
        );
    }

    #[tokio::test]
    async fn tenant_oidc_cache_builds_once_and_invalidate_drops_it() {
        // `from_config` requires network discovery, so this test exercises
        // cache keying/reuse/invalidate directly against a pre-built runtime
        // rather than going through `get_or_build` (which is exercised only
        // by the ignored integration-style test below).
        let cache = TenantOidcCache::new();
        let tenant = TenantId(7);
        assert!(cache.cache.get(&tenant).await.is_none());

        // Simulate what `get_or_build` does on a miss.
        let rt = Arc::new(fake_runtime());
        cache.cache.insert(tenant, rt.clone()).await;
        assert!(cache.cache.get(&tenant).await.is_some());

        cache.invalidate(tenant).await;
        assert!(
            cache.cache.get(&tenant).await.is_none(),
            "invalidate must drop the cached runtime"
        );
    }

    /// Builds an `OidcRuntime` without any network access, for cache tests
    /// that only need *a* runtime value, not a correctly-discovered one.
    fn fake_runtime() -> OidcRuntime {
        OidcRuntime {
            config: cfg(),
            client: reqwest::Client::new(),
            discovery: Discovery {
                authorization_endpoint: "https://idp.example/authorize".into(),
                token_endpoint: "https://idp.example/token".into(),
                jwks_uri: "https://idp.example/jwks".into(),
            },
            jwks: tokio::sync::RwLock::new(Jwks { keys: Vec::new() }),
        }
    }

    #[ignore = "hits the network (discovery + JWKS); run explicitly against a real/test IdP"]
    #[tokio::test]
    async fn from_config_builds_runtime_from_tenant_config() {
        let cfg = TenantOidcConfig {
            tenant_id: TenantId(1),
            issuer: "https://idp.example".into(),
            client_id: "quark".into(),
            client_secret: "secret".into(),
            scopes: vec!["openid".into(), "profile".into()],
            admin_claim: "groups".into(),
            admin_value: "quark-admins".into(),
            readonly_value: String::new(),
            required_value: None,
            post_login_url: None,
        };
        let rt = OidcRuntime::from_config(&cfg).await.unwrap();
        assert_eq!(rt.config.scopes, "openid profile");
    }

    #[test]
    fn map_scopes_is_default_closed() {
        let c = cfg();
        // admin group -> Full
        let admin = serde_json::json!({ "groups": ["x", "quark-admins"] });
        assert_eq!(map_scopes(&admin, &c), vec![Scope::Full]);
        // read-only group -> read scopes
        let ro = serde_json::json!({ "groups": ["quark-viewers"] });
        assert_eq!(
            map_scopes(&ro, &c),
            vec![Scope::LinksRead, Scope::Analytics]
        );
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

    #[tokio::test]
    async fn ensure_user_and_membership_creates_once_and_sets_role() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::lmdb::LmdbStore::open_with_node_id(dir.path(), None).unwrap();

        // First login with the admin group -> creates the user, Admin membership.
        let id1 = ensure_user_and_membership(
            &store,
            false,
            "sub-1",
            "sub1@example.com",
            "Sub One",
            &[Scope::Full],
            None,
        )
        .await
        .unwrap();
        let user = store.get_user_by_subject("sub-1").await.unwrap().unwrap();
        assert_eq!(user.id, id1);
        let membership = store
            .get_membership(id1, DEFAULT_TENANT)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(membership.role, Role::Admin);

        // Second login (e.g. readonly this time) does not create a duplicate
        // user, but does refresh the membership role to match the new scopes.
        let id2 = ensure_user_and_membership(
            &store,
            false,
            "sub-1",
            "sub1@example.com",
            "Sub One",
            &[Scope::LinksRead, Scope::Analytics],
            None,
        )
        .await
        .unwrap();
        assert_eq!(id2, id1);
        let membership = store
            .get_membership(id1, DEFAULT_TENANT)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(membership.role, Role::Viewer);

        // A different subject gets its own user id.
        let id3 = ensure_user_and_membership(
            &store,
            false,
            "sub-2",
            "sub2@example.com",
            "Sub Two",
            &[Scope::LinksRead, Scope::Analytics],
            None,
        )
        .await
        .unwrap();
        assert_ne!(id3, id1);
        let membership = store
            .get_membership(id3, DEFAULT_TENANT)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(membership.role, Role::Viewer);
    }

    // Cloud mode: login upserts the User but creates NO membership in the
    // default tenant — a cloud user starts with 0 memberships until they
    // create or are invited to a workspace (P2b/P2c).
    #[tokio::test]
    async fn cloud_login_creates_user_but_no_default_membership() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::lmdb::LmdbStore::open_with_node_id(dir.path(), None).unwrap();
        let uid =
            ensure_user_and_membership(&store, true, "sub-cloud", "e@x", "E", &[Scope::Full], None)
                .await
                .unwrap();
        assert!(store
            .get_user_by_subject("sub-cloud")
            .await
            .unwrap()
            .is_some());
        // no membership was created in the default tenant
        assert!(store
            .list_memberships_for_user(uid)
            .await
            .unwrap()
            .is_empty());
    }

    // Cloud, per-tenant login (multi-tenancy P2d): passing `tenant_membership`
    // creates the Membership in that tenant with the given role — this is how
    // signing in through a tenant's own IdP joins the tenant.
    #[tokio::test]
    async fn cloud_login_with_tenant_creates_membership_with_claim_role() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::lmdb::LmdbStore::open_with_node_id(dir.path(), None).unwrap();
        let tenant = TenantId(9);

        let uid = ensure_user_and_membership(
            &store,
            true,
            "sub-tenant-admin",
            "e@x",
            "E",
            &[],
            Some((tenant, Role::Admin)),
        )
        .await
        .unwrap();

        let membership = store.get_membership(uid, tenant).await.unwrap().unwrap();
        assert_eq!(membership.role, Role::Admin);
        // Still no membership in the default tenant from this login.
        assert!(store
            .get_membership(uid, DEFAULT_TENANT)
            .await
            .unwrap()
            .is_none());

        // A second login with a different claim-mapped role updates the role
        // rather than duplicating the membership.
        let uid2 = ensure_user_and_membership(
            &store,
            true,
            "sub-tenant-admin",
            "e@x",
            "E",
            &[],
            Some((tenant, Role::Viewer)),
        )
        .await
        .unwrap();
        assert_eq!(uid2, uid);
        let membership = store.get_membership(uid, tenant).await.unwrap().unwrap();
        assert_eq!(membership.role, Role::Viewer);
    }

    #[test]
    fn claim_role_maps_admin_and_readonly_and_defaults_to_member() {
        let cfg = TenantOidcConfig {
            tenant_id: TenantId(1),
            issuer: "https://idp.acme.example".into(),
            client_id: "acme".into(),
            client_secret: "s".into(),
            scopes: vec!["openid".into()],
            admin_claim: "groups".into(),
            admin_value: "acme-admins".into(),
            readonly_value: "acme-viewers".into(),
            required_value: None,
            post_login_url: None,
        };

        let admin = serde_json::json!({ "groups": ["x", "acme-admins"] });
        assert_eq!(claim_role(&admin, &cfg), Role::Admin);

        let ro = serde_json::json!({ "groups": ["acme-viewers"] });
        assert_eq!(claim_role(&ro, &cfg), Role::Viewer);

        // string claim form
        let admin_str = serde_json::json!({ "groups": "acme-admins" });
        assert_eq!(claim_role(&admin_str, &cfg), Role::Admin);

        // neither value present -> Member (not empty, unlike map_scopes)
        let none = serde_json::json!({ "groups": ["random"] });
        assert_eq!(claim_role(&none, &cfg), Role::Member);

        // missing claim entirely -> Member
        let missing = serde_json::json!({ "sub": "x" });
        assert_eq!(claim_role(&missing, &cfg), Role::Member);

        // Owner is never granted by a claim, no matter what the claim says.
        for claims in [&admin, &ro, &admin_str, &none, &missing] {
            assert_ne!(claim_role(claims, &cfg), Role::Owner);
        }
    }

    /// Security sweep (multi-tenancy P2e Task 4): `claim_role` against the
    /// literal group names `provision_tenant_keycloak` writes into every
    /// auto-provisioned tenant's `oidc_config` (`quark-admins`/
    /// `quark-readers`, as opposed to the arbitrary `acme-*` names in
    /// `claim_role_maps_admin_and_readonly_and_defaults_to_member` above) —
    /// `quark-admins` maps to Admin, `quark-readers` to Viewer, and Owner is
    /// never reachable through either.
    #[test]
    fn claim_role_with_provisioned_default_groups_never_grants_owner() {
        let cfg = TenantOidcConfig {
            tenant_id: TenantId(1),
            issuer: "https://kc.example.com/realms/acme".into(),
            client_id: "quark".into(),
            client_secret: String::new(),
            scopes: vec!["openid".into()],
            admin_claim: "groups".into(),
            admin_value: "quark-admins".into(),
            readonly_value: "quark-readers".into(),
            required_value: Some("quark-readers".into()),
            post_login_url: None,
        };

        let admin = serde_json::json!({ "groups": ["quark-admins"] });
        assert_eq!(claim_role(&admin, &cfg), Role::Admin);

        let reader = serde_json::json!({ "groups": ["quark-readers"] });
        assert_eq!(claim_role(&reader, &cfg), Role::Viewer);

        for claims in [&admin, &reader] {
            assert_ne!(claim_role(claims, &cfg), Role::Owner);
        }
    }

    fn cfg_with_required(required_value: Option<&str>) -> TenantOidcConfig {
        TenantOidcConfig {
            tenant_id: TenantId(1),
            issuer: "https://idp.acme.example".into(),
            client_id: "acme".into(),
            client_secret: "s".into(),
            scopes: vec!["openid".into()],
            admin_claim: "groups".into(),
            admin_value: "acme-admins".into(),
            readonly_value: "acme-viewers".into(),
            required_value: required_value.map(str::to_string),
            post_login_url: None,
        }
    }

    // Without `required_value` set, the gate is open: any authenticated
    // tenant IdP user is admitted, matching the pre-Task-4b behavior exactly.
    #[test]
    fn passes_required_group_is_open_when_unset() {
        let cfg = cfg_with_required(None);
        let none = serde_json::json!({ "groups": ["random"] });
        assert!(passes_required_group(&none, &cfg));

        // Empty string is treated the same as unset (default-open), not as
        // "required group is the empty string".
        let cfg_empty = cfg_with_required(Some(""));
        assert!(passes_required_group(&none, &cfg_empty));
    }

    // With `required_value` set, admin/readonly members pass the gate (their
    // claim already satisfies it independent of the required group), a
    // member of the required group passes, and anyone in none of the three
    // is denied.
    #[test]
    fn passes_required_group_is_closed_when_set() {
        let cfg = cfg_with_required(Some("acme-contractors"));

        let admin = serde_json::json!({ "groups": ["acme-admins"] });
        assert!(passes_required_group(&admin, &cfg));

        let readonly = serde_json::json!({ "groups": ["acme-viewers"] });
        assert!(passes_required_group(&readonly, &cfg));

        let required = serde_json::json!({ "groups": ["acme-contractors"] });
        assert!(passes_required_group(&required, &cfg));

        let neither = serde_json::json!({ "groups": ["random"] });
        assert!(!passes_required_group(&neither, &cfg));

        let missing_claim = serde_json::json!({ "sub": "x" });
        assert!(!passes_required_group(&missing_claim, &cfg));

        // Exact match only, never substring: a group that merely contains
        // the required value as a substring must not pass.
        let substring = serde_json::json!({ "groups": ["acme-contractors-alumni"] });
        assert!(!passes_required_group(&substring, &cfg));
    }
}
