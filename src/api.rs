use crate::abuse::{extract_host, is_internal_host};
use crate::analytics::{device_from_ua, AnalyticsSink, ClickEvent};
use crate::auth::{generate_token, hash_token, ApiToken, Scope};
use crate::cache::Cache;
use crate::dns::Dns;
use crate::domain::{Domain, DomainStatus, SHARED_DOMAIN_ID};
use crate::pixel::{PixelConfig, PixelCredentials, Provider};
use crate::sso::{normalize_email_domain, SsoEmailDomain};
use crate::store::{
    matched_rule_index, normalize_folder, normalize_tags, pick_variant, LinkHealth, Record, Rule,
    RuleField, Store, StoreError, Variant,
};
use crate::webhooks::delivery::WebhookDispatcher;
use crate::webhooks::{self, EventType, SubscriptionKind, WebhookEvent, WebhookSubscription};
use crate::{codec, now, permute};
use axum::body::Bytes;
use axum::extract::{ConnectInfo, Path, Query, RawQuery, Request, State};
use axum::http::Method;
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;
use tower_http::cors::CorsLayer;

pub struct AppState {
    pub cache: Cache,
    pub store: Arc<dyn Store>,
    pub key: u64,
    /// Dedicated 32-byte secret for signing unlock cookies (link passwords).
    /// Kept separate from `key` (the 64-bit code-permutation key) so the MAC
    /// secret has full entropy and no shared purpose with the public codec.
    pub signing_key: [u8; 32],
    pub analytics_tx: tokio::sync::mpsc::Sender<ClickEvent>,
    pub sink: Arc<dyn AnalyticsSink>,
    pub admin_token: Option<String>,
    pub ratelimiter: crate::abuse::ratelimit::RateLimiter,
    pub block_private: bool,
    pub public_host: Option<String>,
    pub real_ip_header: String,
    pub webhooks: Arc<WebhookDispatcher>,
    /// OIDC login runtime, present only when OIDC is configured AND initialized.
    pub oidc: Option<Arc<crate::oidc::OidcRuntime>>,
    /// Whether OIDC was configured at all (`QUARK_OIDC_ISSUER` set), independent
    /// of whether init succeeded. Gates the "public shortener" fallback so a
    /// failed IdP init on an OIDC-only deploy fails closed, not open.
    pub oidc_configured: bool,
    /// Google Sheets connector config, present only when the connector is
    /// opted in (`QUARK_SHEETS_CLIENT_ID`/`_SECRET`/`_REDIRECT_URL` all set).
    pub sheets: Option<Arc<crate::sheets::SheetsConfig>>,
    /// The Sheets HTTP seam (real `GoogleSheetsApi` in `main`, absent in tests
    /// that never drive a real sync). `None` is treated as "connector off".
    pub sheets_api: Option<Arc<dyn crate::sheets::client::SheetsApi>>,
    /// Multi-tenant (cloud) mode, from `QUARK_MULTI_TENANT`. Gates FORCE RLS,
    /// per-tenant tx, and (P3 Task 4) whether `redirect`/`unlock` resolve the
    /// `Host` header at all: off, they skip straight to the shared route.
    pub multi_tenant: bool,
    /// Maps a request `Host` header to `{domain_id, tenant_id}` for custom
    /// domains (multi-tenancy P3). In OSS/single-tenant mode every host still
    /// resolves through `public_host` to the shared route. `redirect`/`unlock`
    /// consult this (via `resolve_host_route`) to pick the alias domain and
    /// the tenant the link fetch is scoped by.
    pub host_router: Arc<crate::domain_router::HostRouter>,
    /// TXT lookup seam for custom-domain verification (multi-tenancy P3).
    /// Only `admin_domains_verify` calls it; never on the redirect path.
    pub dns: Arc<dyn Dns>,
    /// Base suffix for the auto per-tenant subdomain (multi-tenancy P3-completion),
    /// e.g. `quarkus.com.br` from `QUARK_TENANT_DOMAIN_SUFFIX`. Cloud-only; `None`
    /// disables the whole subdomain-auto feature (no seed on create, no boot
    /// backfill, `/admin/me` reports `null`).
    pub tenant_domain_suffix: Option<String>,
    /// Per-tenant `OidcRuntime` cache (multi-tenancy P2d): each cloud tenant's
    /// own IdP config (`oidc_configs`) is built into a runtime lazily on first
    /// login and cached here, keyed by tenant id. Invalidated (best-effort) by
    /// `admin_oidc_config_put`/`_delete`; also self-expires via TTL.
    pub oidc_tenants: crate::oidc::TenantOidcCache,
    /// Keycloak admin runtime (multi-tenancy P2e), present only when
    /// `QUARK_KEYCLOAK_BASE_URL` is configured. `None` disables the whole
    /// feature; provisioning logic that calls this is Task 2, not built here.
    pub keycloak: Option<Arc<dyn crate::keycloak::KeycloakAdmin>>,
    /// Base URL Keycloak is reachable at, kept alongside `keycloak` so a
    /// tenant's issuer can be derived (`keycloak::derive_issuer`) without
    /// re-reading the environment.
    pub keycloak_base_url: Option<String>,
}

#[derive(Deserialize)]
pub struct CreateReq {
    url: String,
    alias: Option<String>,
    ttl: Option<u64>,
    tags: Option<Vec<String>>,
    max_visits: Option<u32>,
    rules: Option<Vec<Rule>>,
    variants: Option<Vec<Variant>>,
    app_ios: Option<String>,
    app_android: Option<String>,
    folder: Option<String>,
    fallback_url: Option<String>,
    password: Option<String>,
}

/// Normalizes the `max_visits` request field into the persisted representation:
/// `0` or absent means unlimited (`None`); any `n > 0` is `Some(n)`.
fn normalize_max_visits(raw: Option<u32>) -> Option<u32> {
    raw.filter(|&n| n > 0)
}

/// Maximum number of geo/device rules a single link may carry.
const MAX_RULES: usize = 20;

/// Validates and normalizes rules for `create`/`admin_link_patch`: caps the
/// count, normalizes country codes to uppercase and device values to the
/// canonical `Mobile`/`Desktop`/`Other` set, and runs each rule's `to`
/// through the SAME SSRF guard as the link's main `url` (a rule
/// destination must not smuggle an internal/self host).
async fn validate_rules(
    rules: Vec<Rule>,
    headers: &HeaderMap,
    st: &AppState,
) -> Result<Vec<Rule>, Response> {
    if rules.len() > MAX_RULES {
        return Err((StatusCode::BAD_REQUEST, "too many rules").into_response());
    }
    let mut out = Vec::with_capacity(rules.len());
    for mut rule in rules {
        match rule.field {
            RuleField::Country => {
                rule.values = rule.values.iter().map(|v| v.to_ascii_uppercase()).collect();
            }
            RuleField::Device => {
                let mut normalized = Vec::with_capacity(rule.values.len());
                for v in &rule.values {
                    match ["Mobile", "Desktop", "Other"]
                        .into_iter()
                        .find(|c| c.eq_ignore_ascii_case(v))
                    {
                        Some(c) => normalized.push(c.to_string()),
                        None => {
                            return Err(
                                (StatusCode::BAD_REQUEST, "invalid device value").into_response()
                            )
                        }
                    }
                }
                rule.values = normalized;
            }
        }
        if !is_valid_url(&rule.to) {
            return Err((StatusCode::BAD_REQUEST, "invalid rule destination").into_response());
        }
        let Some(host) = extract_host(&rule.to) else {
            return Err((StatusCode::BAD_REQUEST, "rule destination without host").into_response());
        };
        if st.block_private && is_blocked_target(&host, headers, st).await {
            return Err((StatusCode::FORBIDDEN, "rule destination not allowed").into_response());
        }
        out.push(rule);
    }
    Ok(out)
}

#[derive(Serialize)]
pub struct CreateResp {
    code: String,
    url: String,
}

fn is_valid_url(u: &str) -> bool {
    u.starts_with("http://") || u.starts_with("https://")
}

const DEFAULT_MAX_AGE: u64 = 86400;
/// Default page size for admin listing/search endpoints when `limit` is not provided.
const DEFAULT_PAGE_LIMIT: usize = 50;
/// Maximum page size accepted for admin listing/search endpoints (clamp ceiling).
const MAX_PAGE_LIMIT: usize = 500;
/// Maximum number of webhook subscriptions a deployment may register.
const MAX_WEBHOOK_SUBSCRIPTIONS: usize = 50;
/// Timeout for the synchronous one-shot delivery used by the "test" endpoint.
const WEBHOOK_TEST_TIMEOUT_SECS: u64 = 5;

/// A random id embedded in an outbound event payload's `id` field.
/// Distinct from the `webhook-id` header the delivery worker assigns per
/// attempt (see `webhooks::delivery::deliver_one`): this one identifies the
/// event as recorded at emission time, before it is queued for delivery.
fn generate_event_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("system RNG must be available");
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("evt_{hex}")
}

/// A stable per-click id (`clk_` + 16 random bytes hex), generated once when a
/// redirect captures a click. Mirrors `generate_event_id` / the webhook
/// `generate_msg_id`. Carried on the `ClickEvent` through the analytics channel
/// so a future at-least-once retry sends the same id, which Meta uses to
/// deduplicate the conversion.
fn generate_click_id() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        return String::new();
    }
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("clk_{hex}")
}

/// Builds the JSON body for an outbound webhook event, per the schema in
/// `docs/specs/2026-07-13-webhooks-design.md`: `{id, type, timestamp, data}`,
/// where `data` carries `code`, `url`, an optional `alias`, an optional
/// `expiry`, and `created`. Optional fields are omitted (not `null`) when absent.
///
/// `click` is `Some` only for `link.clicked`: per the design doc, that
/// event's `data` additionally carries the click context already captured
/// for analytics (`country`, `device`, `referrer`, `ts`), reusing the
/// existing `ClickEvent` shape. `country`/`referrer` are omitted when
/// absent (same omit-empty convention as `alias`/`expiry`); `device` is
/// always present, derived from `user_agent` via `device_from_ua` (falls
/// back to `"Other"`).
fn webhook_event_payload(
    event_type: EventType,
    code: &str,
    url: &str,
    alias: Option<&str>,
    expiry: Option<u64>,
    created: u64,
    click: Option<&ClickEvent>,
) -> String {
    let mut data = serde_json::Map::new();
    data.insert(
        "code".to_string(),
        serde_json::Value::String(code.to_string()),
    );
    data.insert(
        "url".to_string(),
        serde_json::Value::String(url.to_string()),
    );
    if let Some(a) = alias {
        data.insert(
            "alias".to_string(),
            serde_json::Value::String(a.to_string()),
        );
    }
    if let Some(e) = expiry {
        data.insert("expiry".to_string(), serde_json::Value::from(e));
    }
    data.insert("created".to_string(), serde_json::Value::from(created));
    if let Some(ev) = click {
        if let Some(c) = &ev.country {
            data.insert("country".to_string(), serde_json::Value::String(c.clone()));
        }
        data.insert(
            "device".to_string(),
            serde_json::Value::String(device_from_ua(ev.user_agent.as_deref()).to_string()),
        );
        if let Some(r) = &ev.referer {
            data.insert("referrer".to_string(), serde_json::Value::String(r.clone()));
        }
        data.insert("ts".to_string(), serde_json::Value::from(ev.ts));
    }
    serde_json::json!({
        "id": generate_event_id(),
        "type": event_type.as_str(),
        "timestamp": now(),
        "data": serde_json::Value::Object(data),
    })
    .to_string()
}

/// Validates a webhook subscription's target URL: must be http/https and must
/// not resolve to an internal/loopback host (SSRF guard, reused from the link
/// destination checks).
fn validate_webhook_url(url: &str) -> Result<(), (StatusCode, &'static str)> {
    if !is_valid_url(url) {
        return Err((StatusCode::BAD_REQUEST, "invalid url"));
    }
    let Some(host) = extract_host(url) else {
        return Err((StatusCode::BAD_REQUEST, "url without host"));
    };
    if is_internal_host(&host) {
        return Err((StatusCode::BAD_REQUEST, "internal destination not allowed"));
    }
    Ok(())
}

/// Masks a webhook secret for display: the raw value is only ever returned
/// once, at creation time. Channel kinds (Slack/Discord/Telegram) carry no
/// signing secret at all, so an empty `secret` masks to an empty string
/// rather than a fake-looking `whsec_••••`.
fn mask_secret(secret: &str) -> String {
    if secret.is_empty() {
        String::new()
    } else {
        "whsec_••••".to_string()
    }
}

/// Cap on the number of A/B variants a single link may have.
const MAX_VARIANTS: usize = 10;

/// Validates a set of A/B variants against the same rules as the main `url`:
/// count cap, `is_valid_url`, SSRF guard (`is_blocked_target`), and a minimum
/// weight of 1. Shared by `create` and `admin_link_patch` so the two paths can
/// never drift out of sync on SSRF coverage.
async fn validate_variants(
    variants: &[Variant],
    headers: &HeaderMap,
    st: &AppState,
) -> Result<(), Response> {
    if variants.len() > MAX_VARIANTS {
        return Err((StatusCode::BAD_REQUEST, "too many variants").into_response());
    }
    for variant in variants {
        if variant.weight < 1 {
            return Err((StatusCode::BAD_REQUEST, "variant weight must be >= 1").into_response());
        }
        if !is_valid_url(&variant.url) {
            return Err((StatusCode::BAD_REQUEST, "invalid variant url").into_response());
        }
        let Some(host) = extract_host(&variant.url) else {
            return Err((StatusCode::BAD_REQUEST, "variant url without host").into_response());
        };
        if st.block_private && is_blocked_target(&host, headers, st).await {
            return Err((StatusCode::FORBIDDEN, "variant destination not allowed").into_response());
        }
    }
    Ok(())
}

/// Document names accepted on the well-known routes (exact, no others).
const WELLKNOWN_NAMES: [&str; 2] = ["apple-app-site-association", "assetlinks.json"];
/// Maximum accepted body size for a well-known document (64 KiB).
const WELLKNOWN_MAX: usize = 65536;

/// Computes the Cache-Control header value for a redirect response,
/// respecting the link's TTL: never caches past expiry. Pure function,
/// a TDD target.
fn cache_control_for(expiry: Option<u64>, now: u64) -> String {
    match expiry {
        None => format!("public, max-age={}", DEFAULT_MAX_AGE),
        Some(e) if e > now => {
            let max_age = DEFAULT_MAX_AGE.min(e - now);
            format!("public, max-age={}", max_age)
        }
        Some(_) => "no-store".to_string(),
    }
}

/// Requires the admin token to create — but only when a token is configured.
/// Without QUARK_ADMIN_TOKEN, create remains public (open shortener).
async fn require_admin_for_create(
    st: &AppState,
    headers: &HeaderMap,
) -> Result<Principal, StatusCode> {
    // Open shortener: create stays public ONLY when no auth mechanism is
    // configured. When either a token or OIDC is set, create requires a
    // credential covering LinksWrite (env token / API token / OIDC session),
    // reusing the same authorization as every other write.
    if st.admin_token.is_none() && !st.oidc_configured {
        return Ok(Principal {
            tenant: crate::tenant::DEFAULT_TENANT,
            user_id: None,
            scopes: vec![Scope::Full],
        });
    }
    admin_guard(st, headers, Scope::LinksWrite).await
}

/// Reasons `create_link_core` can fail. The `create` handler and the
/// `/admin/import` handler both map this to a response: `create` picks an
/// HTTP status, `admin_import` picks a human-readable failure reason string.
#[derive(Debug, PartialEq, Eq)]
pub enum CreateError {
    InvalidUrl,
    NoHost,
    Blocked,
    AliasCollision,
    AliasInUse,
    InvalidTtl,
    IdExhausted,
    Backend,
}

/// Core link-creation logic shared by `POST /` and `POST /admin/import`:
/// validates the URL, runs the SSRF/anti-loop guard, computes expiry
/// from `ttl`, then either claims `alias` (custom code) or allocates the
/// next numeric id. Holds no admin/rate-limit policy — callers apply those
/// before invoking this. Returns the created code (the alias, or the
/// computed base62 code) on success.
///
/// Takes each per-feature field (`tags`, `max_visits`, `rules`, …) as its own
/// parameter: this is the single creation chokepoint every roadmap feature
/// threads its new `Record` field through, so the argument list grows with the
/// data model by design.
#[allow(clippy::too_many_arguments)]
pub async fn create_link_core(
    st: &AppState,
    tenant: crate::tenant::TenantId,
    domain_id: u64,
    url: &str,
    alias: Option<&str>,
    ttl: Option<u64>,
    tags: Vec<String>,
    max_visits: Option<u32>,
    rules: Vec<Rule>,
    variants: Vec<Variant>,
    app_ios: Option<String>,
    app_android: Option<String>,
    folder: Option<String>,
    fallback_url: Option<String>,
    password_hash: Option<String>,
    headers: &HeaderMap,
) -> Result<String, CreateError> {
    if !is_valid_url(url) {
        return Err(CreateError::InvalidUrl);
    }
    let Some(host) = extract_host(url) else {
        return Err(CreateError::NoHost);
    };
    if st.block_private && is_blocked_target(&host, headers, st).await {
        return Err(CreateError::Blocked);
    }

    let expiry = match ttl {
        Some(t) => match now().checked_add(t) {
            Some(e) => Some(e),
            None => return Err(CreateError::InvalidTtl),
        },
        None => None,
    };
    let rec = Record {
        url: url.to_string(),
        expiry,
        created: now(),
        tags,
        max_visits,
        rules,
        variants,
        app_ios,
        app_android,
        folder,
        fallback_url,
        password_hash,
        tenant_id: tenant,
    };

    if let Some(alias) = alias {
        if codec::from_base62(alias).is_some() {
            return Err(CreateError::AliasCollision);
        }
        let id = match st.store.next_id(tenant).await {
            Ok(id) => id,
            Err(StoreError::IdSpaceExhausted) => return Err(CreateError::IdExhausted),
            Err(_) => return Err(CreateError::Backend),
        };
        let canonical_code = codec::to_base62(permute::encode(id, st.key));
        let ev = WebhookEvent {
            event_type: EventType::LinkCreated,
            body: webhook_event_payload(
                EventType::LinkCreated,
                &canonical_code,
                &rec.url,
                Some(alias),
                rec.expiry,
                rec.created,
                None,
            ),
        };
        let rows = st.webhooks.lifecycle_deliveries(tenant, &ev).await;
        match st
            .store
            .put_alias_and_link_tx(tenant, domain_id, alias, id, &rec, &rows)
            .await
        {
            Ok(true) => {}
            Ok(false) => return Err(CreateError::AliasInUse),
            Err(_) => return Err(CreateError::Backend),
        };
        st.webhooks.emit_if_in_memory(ev);
        return Ok(alias.to_string());
    }

    let id = match st.store.next_id(tenant).await {
        Ok(id) => id,
        Err(StoreError::IdSpaceExhausted) => return Err(CreateError::IdExhausted),
        Err(_) => return Err(CreateError::Backend),
    };
    if id > permute::MAX_ID {
        return Err(CreateError::IdExhausted);
    }
    let code = codec::to_base62(permute::encode(id, st.key));
    let ev = WebhookEvent {
        event_type: EventType::LinkCreated,
        body: webhook_event_payload(
            EventType::LinkCreated,
            &code,
            &rec.url,
            None,
            rec.expiry,
            rec.created,
            None,
        ),
    };
    let rows = st.webhooks.lifecycle_deliveries(tenant, &ev).await;
    if st.store.put_link_tx(tenant, id, &rec, &rows).await.is_err() {
        return Err(CreateError::Backend);
    }
    st.webhooks.emit_if_in_memory(ev);
    Ok(code)
}

/// Maps a `CreateError` to the exact HTTP status/body `create` has always
/// returned for it.
fn create_error_response(err: CreateError) -> Response {
    match err {
        CreateError::InvalidUrl => (StatusCode::BAD_REQUEST, "invalid url").into_response(),
        CreateError::NoHost => (StatusCode::BAD_REQUEST, "url without host").into_response(),
        CreateError::Blocked => (StatusCode::FORBIDDEN, "blocked destination").into_response(),
        CreateError::AliasCollision => (
            StatusCode::BAD_REQUEST,
            "alias collides with the numeric code space",
        )
            .into_response(),
        CreateError::AliasInUse => (StatusCode::CONFLICT, "alias in use").into_response(),
        CreateError::InvalidTtl => (StatusCode::BAD_REQUEST, "invalid ttl").into_response(),
        CreateError::IdExhausted => {
            (StatusCode::INSUFFICIENT_STORAGE, "id space exhausted").into_response()
        }
        CreateError::Backend => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// A short, human-readable reason for an import row failure (used in the
/// `/admin/import` summary's `failed[].reason`).
fn create_error_reason(err: &CreateError) -> &'static str {
    match err {
        CreateError::InvalidUrl => "invalid url",
        CreateError::NoHost => "url without host",
        CreateError::Blocked => "blocked destination",
        CreateError::AliasCollision => "alias collides with the numeric code space",
        CreateError::AliasInUse => "alias in use",
        CreateError::InvalidTtl => "invalid ttl",
        CreateError::IdExhausted => "id space exhausted",
        CreateError::Backend => "backend error",
    }
}

/// The `domains` row a newly created link's alias should land in: on cloud,
/// with a suffix configured and a real (non-default) tenant, the tenant's
/// own subdomain — so `<slug>.<suffix>/<alias>` resolves. Any miss (OSS,
/// suffix unset, `DEFAULT_TENANT`, tenant/domain row not found) falls back
/// to `SHARED_DOMAIN_ID`, which is also the OSS byte-for-byte behavior.
async fn default_domain_id(st: &AppState, tenant: crate::tenant::TenantId) -> u64 {
    if !st.multi_tenant || tenant == crate::tenant::DEFAULT_TENANT {
        return SHARED_DOMAIN_ID;
    }
    let Some(suffix) = st.tenant_domain_suffix.as_deref() else {
        return SHARED_DOMAIN_ID;
    };
    let Ok(Some(t)) = st.store.get_tenant(tenant).await else {
        return SHARED_DOMAIN_ID;
    };
    let host = subdomain_host(&t.slug, suffix);
    match st.store.get_domain_by_host(&host).await {
        Ok(Some(domain)) => domain.id,
        _ => SHARED_DOMAIN_ID,
    }
}

async fn create(
    State(st): State<Arc<AppState>>,
    conn: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    Json(req): Json<CreateReq>,
) -> Response {
    let p = match require_admin_for_create(&st, &headers).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    let ip = client_ip(&headers, &st.real_ip_header, conn.as_ref());
    if !st.ratelimiter.check(&ip, now()).await {
        return (StatusCode::TOO_MANY_REQUESTS, "too many requests").into_response();
    }
    let rules = match validate_rules(req.rules.clone().unwrap_or_default(), &headers, &st).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let variants = req.variants.clone().unwrap_or_default();
    if let Err(resp) = validate_variants(&variants, &headers, &st).await {
        return resp;
    }
    if let Some(app) = req.app_ios.as_deref() {
        if let Err(status) = app_destination_ok(&st, &headers, app).await {
            return (status, "invalid app destination").into_response();
        }
    }
    if let Some(app) = req.app_android.as_deref() {
        if let Err(status) = app_destination_ok(&st, &headers, app).await {
            return (status, "invalid app destination").into_response();
        }
    }
    // Trim + drop empty so an empty field means "no fallback"; validate the
    // destination with the same rules/status codes as the main URL.
    let fallback_url = req
        .fallback_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if let Some(fb) = fallback_url.as_deref() {
        if let Err(status) = app_destination_ok(&st, &headers, fb).await {
            return (status, "invalid fallback url").into_response();
        }
    }
    // Hash a non-empty password; the plaintext is never stored or logged. argon2
    // is memory-hard, so hash off the async worker.
    let password_hash = match req
        .password
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(pw) => {
            let pw = pw.to_string();
            match tokio::task::spawn_blocking(move || crate::password::hash_password(&pw)).await {
                Ok(Ok(h)) => Some(h),
                _ => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, "could not hash password")
                        .into_response()
                }
            }
        }
        None => None,
    };
    let domain_id = default_domain_id(&st, p.tenant).await;
    match create_link_core(
        &st,
        p.tenant,
        domain_id,
        &req.url,
        req.alias.as_deref(),
        req.ttl,
        normalize_tags(req.tags.clone().unwrap_or_default()),
        normalize_max_visits(req.max_visits),
        rules,
        variants,
        req.app_ios.clone(),
        req.app_android.clone(),
        normalize_folder(req.folder.clone()),
        fallback_url,
        password_hash,
        &headers,
    )
    .await
    {
        Ok(code) => Json(CreateResp { code, url: req.url }).into_response(),
        Err(err) => create_error_response(err),
    }
}

#[derive(Serialize)]
struct ImportFailure {
    index: usize,
    url: String,
    reason: String,
}

#[derive(Serialize)]
struct ImportSummary {
    imported: usize,
    failed: Vec<ImportFailure>,
}

/// `POST /admin/import`: bulk-creates links from a CSV or JSON body (see
/// `crate::import`), driving each row through `create_link_core` so
/// validation matches `POST /` exactly. Always admin-gated
/// via `admin_guard`, independent of `require_admin_for_create` (import
/// stays admin-only even when public create is enabled). Never aborts on a
/// bad row: every row is attempted, and failures are reported in the
/// summary instead of failing the request.
async fn admin_import(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let p = match admin_guard(&st, &headers, Scope::LinksWrite).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    // Content-type-sniffed body: a cross-site text/plain POST would parse, so
    // guard it like the other cookie-authable simple-POST endpoints.
    if let Err(status) = csrf_guard(&headers) {
        return status.into_response();
    }
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok());
    let format = crate::import::detect_format(content_type, &body);
    let rows = match crate::import::import_rows(&body, format) {
        Ok(rows) => rows,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid import body").into_response(),
    };
    if rows.len() > crate::import::MAX_IMPORT_ROWS {
        return (StatusCode::BAD_REQUEST, "too many rows").into_response();
    }

    let domain_id = default_domain_id(&st, p.tenant).await;
    let mut imported = 0usize;
    let mut failed = Vec::new();
    for (index, row) in rows.into_iter().enumerate() {
        match create_link_core(
            &st,
            p.tenant,
            domain_id,
            &row.url,
            row.alias.as_deref(),
            row.ttl,
            Vec::new(),
            None,
            Vec::new(),
            Vec::new(),
            None,
            None,
            None,
            None,
            None,
            &headers,
        )
        .await
        {
            Ok(_) => imported += 1,
            Err(err) => failed.push(ImportFailure {
                index,
                url: row.url,
                reason: create_error_reason(&err).to_string(),
            }),
        }
    }
    Json(ImportSummary { imported, failed }).into_response()
}

/// Client IP: configurable header (default CF-Connecting-IP) takes priority;
/// otherwise the socket IP; otherwise "unknown" (single, conservative bucket).
fn client_ip(
    headers: &HeaderMap,
    header_name: &str,
    conn: Option<&ConnectInfo<SocketAddr>>,
) -> String {
    if let Some(v) = headers.get(header_name).and_then(|v| v.to_str().ok()) {
        let v = v.trim();
        if !v.is_empty() {
            return v.to_string();
        }
    }
    if let Some(ConnectInfo(addr)) = conn {
        return addr.ip().to_string();
    }
    "unknown".to_string()
}

/// Click platform inferred from the User-Agent, used to pick an app destination.
/// Consumed by the redirect handler (wired in the device-redirect task).
#[derive(Debug, PartialEq, Eq)]
enum Platform {
    Ios,
    Android,
    Other,
}

/// Classifies a click by User-Agent. Apple device tokens win over Android;
/// anything else (desktop, bots, missing header) is `Other`. Case-sensitive
/// substring match on the raw UA: these vendor tokens are stable.
fn classify_platform(ua: Option<&str>) -> Platform {
    match ua {
        Some(ua) if ua.contains("iPhone") || ua.contains("iPad") || ua.contains("iPod") => {
            Platform::Ios
        }
        Some(ua) if ua.contains("Android") => Platform::Android,
        _ => Platform::Other,
    }
}

/// Resolves the app-specific destination for a click, or `None` when the record
/// has none for the click's platform (the caller then falls back to `rec.url`).
fn app_destination<'a>(rec: &'a Record, ua: Option<&str>) -> Option<&'a str> {
    match classify_platform(ua) {
        Platform::Ios => rec.app_ios.as_deref(),
        Platform::Android => rec.app_android.as_deref(),
        Platform::Other => None,
    }
}

/// Built-in guard: internal network destination, or a loop back to any host
/// quark itself serves (the shared `public_host`, or in multi-tenant mode
/// any verified custom domain — a self-loop through a custom domain is still
/// a self-loop).
async fn is_blocked_target(host: &str, headers: &HeaderMap, st: &AppState) -> bool {
    if is_internal_host(host) {
        return true;
    }
    let self_host = st.public_host.clone().or_else(|| {
        headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .map(|h| h.split(':').next().unwrap_or(h).to_ascii_lowercase())
    });
    if matches!(self_host, Some(sh) if sh == host) {
        return true;
    }
    if st.multi_tenant && st.host_router.resolve(host).await.is_some() {
        return true;
    }
    false
}

/// Validates an app destination URL with the same rules — and the same status
/// codes — as the main link URL: 400 for a malformed URL (bad scheme / no host),
/// 403 for a policy denial (internal/self target). Mirrors the create/patch
/// main-`url` arms exactly, in the same order.
async fn app_destination_ok(
    st: &AppState,
    headers: &HeaderMap,
    url: &str,
) -> Result<(), StatusCode> {
    if !is_valid_url(url) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let Some(host) = extract_host(url) else {
        return Err(StatusCode::BAD_REQUEST);
    };
    if st.block_private && is_blocked_target(&host, headers, st).await {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

/// Resolves a URL code into an id: first tries a numeric code (base62 in the
/// domain); if not, treats it as an alias in the store. `Ok(Some(id))` resolved,
/// `Ok(None)` doesn't exist, `Err` backend failure. Each handler maps these
/// cases to its own HTTP response (the redirect attaches Cache-Control on 404).
///
/// The numeric decode is global (the permutation namespace is not scoped by
/// domain or tenant); `domain_id` only matters for the alias fallback, which
/// is scoped per-domain (`SHARED_DOMAIN_ID` on the shared host, or the id of
/// a resolved custom domain).
async fn resolve_code(
    st: &AppState,
    domain_id: u64,
    code: &str,
) -> Result<Option<u64>, StoreError> {
    match codec::from_base62(code) {
        Some(c) if c <= permute::MAX_ID => Ok(Some(permute::decode(c, st.key))),
        _ => st.store.get_alias(domain_id, code).await,
    }
}

/// Resolves the request's `Host` header into a route for the redirect/unlock
/// hot path.
///
/// OSS (`multi_tenant` off): always the shared route, no `Host` lookup at
/// all — byte-for-byte the pre-P3 behavior, and the only path OSS ever
/// takes since `host_router.resolve` would land on the same shared route
/// anyway.
///
/// Cloud (`multi_tenant` on): normalizes the `Host` header and resolves it
/// through `host_router`. `None` (missing header, or a host the router
/// doesn't recognize — unknown, or a domain still pending verification)
/// means the caller must 404: an unverified/unknown host serves nothing.
async fn resolve_host_route(
    st: &AppState,
    headers: &HeaderMap,
) -> Option<crate::domain::DomainRoute> {
    if !st.multi_tenant {
        return Some(crate::domain::DomainRoute {
            domain_id: SHARED_DOMAIN_ID,
            tenant_id: crate::tenant::DEFAULT_TENANT,
        });
    }
    let host = headers.get(header::HOST)?.to_str().ok()?;
    st.host_router.resolve(host).await
}

/// Extracts and percent-decodes the `fbclid` query parameter Meta ads append
/// to a click URL, if present. Used only to build the Meta `fbc` cookie value
/// for server-side conversion forwarding; never persisted.
fn fbclid_from_query(raw: Option<&str>) -> Option<String> {
    let raw = raw?;
    url::form_urlencoded::parse(raw.as_bytes())
        .find(|(k, _)| k == "fbclid")
        .map(|(_, v)| v.into_owned())
        .filter(|v| !v.is_empty())
}

/// Response for an expired link (whether by time or by visit count): a `302`
/// to the link's configured fallback URL when one is set, otherwise the
/// historical `410 Gone`. Both carry `Cache-Control: no-store` — visit-count
/// expiry is decided per-request, so the response must never be cached.
fn expired_response(fallback: Option<&str>) -> Response {
    match fallback {
        Some(url) => (
            StatusCode::FOUND,
            [
                (header::LOCATION, url.to_string()),
                (header::CACHE_CONTROL, "no-store".to_string()),
            ],
        )
            .into_response(),
        None => (
            StatusCode::GONE,
            [(header::CACHE_CONTROL, "no-store".to_string())],
            "expired link",
        )
            .into_response(),
    }
}

/// Reads the value of cookie `name` from the request `Cookie` header.
fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|kv| {
        let (k, v) = kv.trim().split_once('=')?;
        (k == name).then_some(v)
    })
}

/// Whether the request carries a valid, unexpired unlock cookie for `code`
/// (the link's canonical code) under its current `password_hash`. `key` is the
/// dedicated 32-byte signing secret.
fn is_unlocked(headers: &HeaderMap, key: &[u8], code: &str, password_hash: &str, now: u64) -> bool {
    match cookie_value(headers, &format!("qk_pw_{code}")) {
        Some(v) => crate::password::unlock_token_valid(v, key, code, password_hash, now),
        None => false,
    }
}

/// Whether the original client request arrived over HTTPS, per `X-Forwarded-Proto`
/// (quark runs behind a TLS-terminating proxy/CDN). Absent → treated as plain
/// HTTP so the `Secure` cookie attribute is not set on local/dev HTTP.
fn request_is_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        // Chained proxies produce a comma-list like "https, http"; the original
        // client scheme is the first entry.
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().eq_ignore_ascii_case("https"))
        .unwrap_or(false)
}

/// Minimal HTML-attribute/text escaping for the one untrusted value embedded in
/// the interstitial (the code/alias from the path).
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Extracts a field from an `application/x-www-form-urlencoded` body.
fn form_field(body: &Bytes, name: &str) -> Option<String> {
    url::form_urlencoded::parse(body)
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
}

/// Renders the self-contained password interstitial. No external assets (inline
/// CSS only) so it works on any deployment. Bilingual by a simple
/// `Accept-Language` sniff; `error` shows a generic "wrong password" message.
fn interstitial_html(code: &str, query: Option<&str>, pt: bool, error: bool) -> String {
    // Preserve the original query string on the form's action so params the
    // redirect consumes (e.g. `fbclid` for Meta CAPI) survive the unlock round-trip.
    let action = match query.filter(|q| !q.is_empty()) {
        Some(q) => html_escape(&format!("/{code}?{q}")),
        None => html_escape(&format!("/{code}")),
    };
    let (title, prompt, placeholder, button, err_msg) = if pt {
        (
            "Link protegido",
            "Este link é protegido por senha.",
            "Senha",
            "Acessar",
            "Senha incorreta. Tente de novo.",
        )
    } else {
        (
            "Protected link",
            "This link is password-protected.",
            "Password",
            "Unlock",
            "Wrong password. Try again.",
        )
    };
    let error_block = if error {
        format!(r#"<p class="err" role="alert">{err_msg}</p>"#)
    } else {
        String::new()
    };
    format!(
        r#"<!DOCTYPE html>
<html lang="{lang}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex">
<title>{title}</title>
<style>
:root {{ color-scheme: light dark; }}
* {{ box-sizing: border-box; }}
body {{ margin: 0; min-height: 100vh; display: grid; place-items: center;
  font-family: system-ui, -apple-system, "Segoe UI", Roboto, sans-serif;
  background: #0a0b0f; color: #e8eaf2; padding: 1.5rem; }}
.card {{ width: 100%; max-width: 22rem; background: #14161f; border: 1px solid #262a3a;
  border-radius: 14px; padding: 2rem 1.75rem; box-shadow: 0 10px 40px rgba(0,0,0,.4); }}
h1 {{ font-size: 1.15rem; margin: 0 0 .35rem; }}
p {{ margin: 0 0 1.15rem; color: #9aa0b4; font-size: .92rem; line-height: 1.45; }}
.err {{ color: #ff7a90; }}
label {{ display: block; font-size: .8rem; margin-bottom: .4rem; color: #c3c8db; }}
input {{ width: 100%; padding: .7rem .8rem; border-radius: 9px; border: 1px solid #2c3145;
  background: #0f111a; color: #e8eaf2; font-size: 1rem; }}
input:focus {{ outline: 2px solid #c6f94e; outline-offset: 1px; }}
button {{ width: 100%; margin-top: 1rem; padding: .72rem; border: 0; border-radius: 9px;
  background: #c6f94e; color: #0a0b0f; font-weight: 600; font-size: .98rem; cursor: pointer; }}
button:hover {{ filter: brightness(1.05); }}
</style>
</head>
<body>
<main class="card">
<h1>{title}</h1>
<p>{prompt}</p>
{error_block}
<form method="post" action="{action}">
<label for="pw">{placeholder}</label>
<input id="pw" name="password" type="password" autocomplete="current-password" autofocus required>
<button type="submit">{button}</button>
</form>
</main>
</body>
</html>"#,
        lang = if pt { "pt-BR" } else { "en" },
    )
}

/// Builds the `200 text/html` interstitial response (never cached). `query` is
/// the original request query string, preserved onto the form action.
fn interstitial_response(
    code: &str,
    query: Option<&str>,
    headers: &HeaderMap,
    error: bool,
) -> Response {
    let pt = headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_ascii_lowercase().contains("pt"))
        .unwrap_or(false);
    (
        StatusCode::OK,
        [
            (header::CACHE_CONTROL, "no-store".to_string()),
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()),
        ],
        interstitial_html(code, query, pt, error),
    )
        .into_response()
}

/// `POST /:code`: unlock a password-protected link. Rate-limited. On the correct
/// password, sets a signed unlock cookie and redirects (303) back to `GET /:code`
/// so the canonical redirect path does destination resolution, the visit bump,
/// and click recording exactly once. A wrong password re-renders the interstitial
/// with an error and sets no cookie. An unprotected code just bounces to its GET.
async fn unlock(
    State(st): State<Arc<AppState>>,
    Path(code): Path<String>,
    conn: Option<ConnectInfo<SocketAddr>>,
    RawQuery(raw_query): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let ip = client_ip(&headers, &st.real_ip_header, conn.as_ref());
    if !st.ratelimiter.check(&ip, now()).await {
        return (StatusCode::TOO_MANY_REQUESTS, "too many requests").into_response();
    }
    let Some(route) = resolve_host_route(&st, &headers).await else {
        return (
            StatusCode::NOT_FOUND,
            [(header::CACHE_CONTROL, "no-store".to_string())],
        )
            .into_response();
    };
    let id = match resolve_code(&st, route.domain_id, &code).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                [(header::CACHE_CONTROL, "no-store".to_string())],
            )
                .into_response()
        }
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    // The GET path consumes the query string (e.g. `fbclid`); preserve it across
    // the unlock round-trip so protected links keep attribution parity with
    // unprotected ones.
    let location = match raw_query.as_deref().filter(|q| !q.is_empty()) {
        Some(q) => format!("/{code}?{q}"),
        None => format!("/{code}"),
    };
    // Scoped by the resolved tenant: on a custom domain, a link owned by a
    // different tenant is invisible here (cross-tenant isolation), same as
    // in `redirect`.
    let rec = match st.cache.get(route.tenant_id, id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                [(header::CACHE_CONTROL, "no-store".to_string())],
            )
                .into_response()
        }
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let Some(hash) = rec.password_hash.clone() else {
        // Not protected: nothing to unlock, send them to the normal GET.
        return (StatusCode::SEE_OTHER, [(header::LOCATION, location)]).into_response();
    };
    let submitted = form_field(&body, "password").unwrap_or_default();
    // argon2 is memory-hard; verify off the async worker so it never stalls the
    // runtime that serves redirects. Clone the hash into the task and keep the
    // original to bind into the unlock token below.
    let hash_for_verify = hash.clone();
    let ok = tokio::task::spawn_blocking(move || {
        crate::password::verify_password(&submitted, &hash_for_verify)
    })
    .await
    .unwrap_or(false);
    if !ok {
        return interstitial_response(&code, raw_query.as_deref(), &headers, true);
    }
    // Key the unlock cookie to the canonical code (not the path string) with
    // Path=/, so it is honored whether the visitor returns via the alias or the
    // code, and the cookie name is always a safe base62 string. The token is also
    // bound to the current password hash, so rotating the password kills it.
    let canonical = codec::to_base62(permute::encode(id, st.key));
    let (token, _expiry) = crate::password::unlock_token(&st.signing_key, &canonical, &hash, now());
    let secure = if request_is_https(&headers) {
        "; Secure"
    } else {
        ""
    };
    let cookie = format!(
        "qk_pw_{canonical}={token}; Max-Age={}; Path=/; HttpOnly; SameSite=Lax{secure}",
        crate::password::UNLOCK_TTL_SECS,
    );
    (
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, location),
            (header::SET_COOKIE, cookie),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
    )
        .into_response()
}

async fn redirect(
    State(st): State<Arc<AppState>>,
    Path(code): Path<String>,
    conn: Option<ConnectInfo<SocketAddr>>,
    RawQuery(raw_query): RawQuery,
    headers: HeaderMap,
) -> Response {
    let Some(route) = resolve_host_route(&st, &headers).await else {
        return (
            StatusCode::NOT_FOUND,
            [(header::CACHE_CONTROL, "no-store".to_string())],
        )
            .into_response();
    };
    let id = match resolve_code(&st, route.domain_id, &code).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                [(header::CACHE_CONTROL, "no-store".to_string())],
            )
                .into_response()
        }
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    // Scoped by the resolved tenant: on a custom domain, a link owned by a
    // different tenant is invisible here (the store's `get_link` filters by
    // tenant) — this is the cross-tenant isolation filter. On the shared host
    // `route.tenant_id` is always `DEFAULT_TENANT`, matching pre-P3 behavior.
    match st.cache.get(route.tenant_id, id).await {
        Ok(Some(mut rec)) => {
            let now = now();
            if let Some(exp) = rec.expiry {
                if now >= exp {
                    if st.webhooks.expired_subscribed.load(Ordering::Relaxed) {
                        st.webhooks.emit(WebhookEvent {
                            event_type: EventType::LinkExpired,
                            body: webhook_event_payload(
                                EventType::LinkExpired,
                                &code,
                                &rec.url,
                                None,
                                rec.expiry,
                                rec.created,
                                None,
                            ),
                        });
                    }
                    return expired_response(rec.fallback_url.as_deref());
                }
            }
            // Password gate: a protected link without a valid unlock cookie shows
            // the interstitial. Placed BEFORE the visit bump so merely viewing the
            // form never consumes a visit; placed AFTER the expiry check so an
            // expired link stays expired regardless of the password. The unlock
            // cookie is keyed to the link's canonical code (not the path string),
            // so it works whether the visitor arrived via the alias or the code.
            // `canonical` is computed only here, never on the unprotected hot path.
            if let Some(hash) = rec.password_hash.as_deref() {
                let canonical = codec::to_base62(permute::encode(id, st.key));
                if !is_unlocked(&headers, &st.signing_key, &canonical, hash, now) {
                    return interstitial_response(&code, raw_query.as_deref(), &headers, false);
                }
            }
            if let Some(max) = rec.max_visits {
                // Count the visit against the tenant the link was resolved
                // under (`route.tenant_id`, the same scope `cache.get` used
                // above), not a hardcoded `DEFAULT_TENANT`: on a cloud
                // subdomain the counter must land on the owning tenant. On the
                // shared host / OSS `route.tenant_id` is already
                // `DEFAULT_TENANT`, so this is byte-for-byte the old behavior.
                let n = match st.store.bump_visits(route.tenant_id, id).await {
                    Ok(n) => n,
                    Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
                };
                if n > max as u64 {
                    return expired_response(rec.fallback_url.as_deref());
                }
            }
            // A password-protected link must never be cached by a shared CDN/proxy:
            // a cached 302-to-destination would let visitors who never entered the
            // password follow it. Force `no-store` for protected links.
            let cache_control = if rec.password_hash.is_some() {
                "no-store".to_string()
            } else {
                cache_control_for(rec.expiry, now)
            };

            let country = headers
                .get("cf-ipcountry")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let user_agent = headers
                .get(header::USER_AGENT)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());

            // Destination resolution composes three targeting mechanisms in
            // priority order: (1) device-aware app deep-links (#20) — a
            // visitor on iOS/Android with a matching `app_ios`/`app_android`
            // is the most specific intent and wins; then (2) geo/device
            // redirect rules (#12), a targeted web redirect; then (3) A/B
            // variants (#17), a stateless weighted pick (one getrandom draw,
            // no store write). Links with none of these (the common case)
            // reuse `rec.url` directly. `variant` is only recorded for the A/B
            // branch.
            let app_dest = if rec.app_ios.is_some() || rec.app_android.is_some() {
                app_destination(&rec, user_agent.as_deref()).map(str::to_string)
            } else {
                None
            };
            // Read the gate once: the `link.clicked` payload below borrows
            // `rec.url`, so the common path may only MOVE `rec.url` into the
            // location when no subscriber will need it afterwards.
            let clicked_subscribed = st.webhooks.clicked_subscribed.load(Ordering::Relaxed);
            let (location, variant): (String, Option<u32>) = if let Some(app) = app_dest {
                (app, None)
            } else {
                match matched_rule_index(&rec.rules, country.as_deref(), user_agent.as_deref()) {
                    Some(i) => (rec.rules[i].to.clone(), None),
                    None if rec.variants.is_empty() => {
                        // Common case: move `rec.url` out (no allocation) unless
                        // the gated click webhook still needs to read it.
                        if clicked_subscribed {
                            (rec.url.clone(), None)
                        } else {
                            (std::mem::take(&mut rec.url), None)
                        }
                    }
                    None => {
                        let mut buf = [0u8; 8];
                        let r = if getrandom::fill(&mut buf).is_ok() {
                            u64::from_le_bytes(buf)
                        } else {
                            0
                        };
                        let i = pick_variant(&rec.variants, r);
                        (rec.variants[i].url.clone(), Some(i as u32))
                    }
                }
            };

            let ev = ClickEvent {
                id,
                event_id: generate_click_id(),
                ts: now,
                referer: headers
                    .get(header::REFERER)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string()),
                country,
                user_agent,
                city: headers
                    .get("cf-ipcity")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string()),
                bot: false,
                ip: match client_ip(&headers, &st.real_ip_header, conn.as_ref()) {
                    s if s == "unknown" => None,
                    s => Some(s),
                },
                fbc: fbclid_from_query(raw_query.as_deref())
                    .map(|fbclid| format!("fb.1.{}.{}", now.saturating_mul(1000), fbclid)),
                variant,
                tenant_id: rec.tenant_id.0,
            };
            // Gate already read above into `clicked_subscribed`. The payload
            // build reads `ev`'s fields (and `rec.url`, which is only still
            // populated on this branch because the gate was true), so it
            // happens before `ev` is moved into `try_send` below.
            if clicked_subscribed {
                st.webhooks.emit(WebhookEvent {
                    event_type: EventType::LinkClicked,
                    body: webhook_event_payload(
                        EventType::LinkClicked,
                        &code,
                        &rec.url,
                        None,
                        rec.expiry,
                        rec.created,
                        Some(&ev),
                    ),
                });
            }

            let _ = st.analytics_tx.try_send(ev);

            (
                StatusCode::FOUND,
                [
                    (header::LOCATION, location),
                    (header::CACHE_CONTROL, cache_control),
                ],
            )
                .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            [(header::CACHE_CONTROL, "no-store".to_string())],
        )
            .into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

async fn stats(
    State(st): State<Arc<AppState>>,
    Path(code): Path<String>,
    headers: HeaderMap,
) -> Response {
    let p = match admin_guard(&st, &headers, Scope::Analytics).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    // Admin `stats` resolves aliases through the caller's tenant default
    // domain, matching where `create` stamps the alias (subdomain on cloud,
    // `SHARED_DOMAIN_ID` on OSS/default tenant). Numeric codes decode
    // globally regardless, via `resolve_code`'s base62 fast path.
    let domain_id = default_domain_id(&st, p.tenant).await;
    let id = match resolve_code(&st, domain_id, &code).await {
        Ok(Some(id)) => id,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    match st.store.get_link(p.tenant, id).await {
        Ok(Some(_)) => {}
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    match st.sink.stats(id).await {
        Ok(Some(s)) => Json(s).into_response(),
        Ok(None) => Json(crate::analytics::Stats {
            aggregates: crate::analytics::Aggregates::default(),
            recent: Vec::new(),
        })
        .into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// `GET /admin/stats`: aggregate analytics across every link owned by the
/// caller's tenant — the "all my links" view (multi-tenancy P4a Task 2),
/// distinct from `GET /:code/stats`'s single-link view above. There's no
/// link to check ownership of here; the tenant scope comes entirely from
/// `admin_guard`'s `Principal`, which the sink filters on internally. On
/// OSS, `p.tenant` is always `DEFAULT_TENANT`, so this aggregates everything
/// — the existing single-tenant behavior.
async fn admin_stats(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let p = match admin_guard(&st, &headers, Scope::Analytics).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    match st.sink.stats_for_tenant(p.tenant.0).await {
        Ok(agg) => Json(agg).into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// The authenticated caller behind an authorized admin request: which tenant
/// its data access is scoped to, the backing user (when known — OIDC only),
/// and the scopes the credential carries. Resolved by `admin_guard` and
/// threaded into handlers so tenant-owned reads/writes go through the
/// principal's tenant rather than a hardcoded default.
#[derive(Debug)]
pub struct Principal {
    pub tenant: crate::tenant::TenantId,
    pub user_id: Option<u64>,
    pub scopes: Vec<Scope>,
}

/// Authorizes an admin request against a required `Scope`: `Ok(Principal)` if
/// authorized; `Err(status)` otherwise. Returns `StatusCode` (not `Response`)
/// in the error to stay `Copy`/small — avoids clippy's `result_large_err`
/// lint, which would trigger with `Response` in the `Err`.
///
/// Order (exact status contract, must not regress the env-token-only path):
/// 1. The env `QUARK_ADMIN_TOKEN`, compared in constant time, is always
///    `Full` (superuser) and never touches the store — this preserves the
///    pre-existing behavior byte for byte.
/// 2. Otherwise, a non-empty provided token is hashed and looked up as an
///    API token: found + scope covers `required` + (no per-token rate limit,
///    or under it) -> `Ok`; found but insufficient scope -> `403`; found but
///    over its rate limit -> `429`; not found -> `401` if an env token is
///    configured, else `404` (endpoint fully disabled when nothing is set up).
/// 3. An empty provided token (nothing supplied) follows the same not-found
///    rule: `401` if an env token is configured, else `404`.
async fn admin_guard(
    st: &AppState,
    headers: &HeaderMap,
    required: Scope,
) -> Result<Principal, StatusCode> {
    let provided = headers
        .get("x-admin-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    // 1) Break-glass env admin token (always Full).
    if let Some(expected) = st.admin_token.as_deref() {
        if constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
            return Ok(Principal {
                tenant: crate::tenant::DEFAULT_TENANT,
                user_id: None,
                scopes: vec![Scope::Full],
            });
        }
    }

    // Admin surface is "disabled" (404) only when neither a token nor OIDC is
    // configured; otherwise a missing/wrong credential is 401. Keyed on whether
    // OIDC was configured (not on init success) so a failed IdP init still keeps
    // the surface locked.
    let not_found_status = if st.admin_token.is_some() || st.oidc_configured {
        StatusCode::UNAUTHORIZED
    } else {
        StatusCode::NOT_FOUND
    };

    // Try every presented credential; any one that covers `required` authorizes.
    // A valid-but-insufficient credential yields 403, and a covering-but-throttled
    // API token yields 429, but only if NOTHING else covers — so a low-scope or
    // rate-limited API token in localStorage never blocks a sufficient session.
    let mut saw_insufficient = false;
    let mut saw_rate_limited = false;
    // A store error on one credential's lookup must not deny a user who also
    // presents a covering one: record it and 503 only if nothing else authorizes.
    let mut saw_store_error = false;

    // 2) Scoped API token in x-admin-token.
    if !provided.is_empty() {
        let hash = hash_token(provided);
        match st.store.get_api_token_by_hash(&hash).await {
            Ok(Some(token)) => {
                if token.scopes.iter().any(|s| s.covers(required)) {
                    match token.rate_limit_per_min {
                        Some(limit) => {
                            let key = format!("tok:{}", token.id);
                            if st.ratelimiter.check_with_limit(&key, now(), limit).await {
                                return Ok(Principal {
                                    tenant: token.tenant_id,
                                    user_id: None,
                                    scopes: token.scopes.clone(),
                                });
                            }
                            saw_rate_limited = true;
                        }
                        None => {
                            return Ok(Principal {
                                tenant: token.tenant_id,
                                user_id: None,
                                scopes: token.scopes.clone(),
                            })
                        }
                    }
                } else {
                    saw_insufficient = true;
                }
            }
            Ok(None) => {}
            Err(_) => saw_store_error = true,
        }
    }

    // 3) OIDC login session cookie. Only honored when OIDC is configured, so
    // disabling OIDC immediately stops leftover session cookies from
    // authorizing (the session GC only runs while OIDC is on).
    if st.oidc_configured {
        if let Some(raw) = cookie_value(headers, SESSION_COOKIE) {
            let hash = hash_token(raw);
            match st.store.get_session_by_hash(&hash, now()).await {
                Ok(Some(session)) => {
                    // Where the session's authorization comes from differs by
                    // deployment mode. OSS: the stored `session.scopes`, which
                    // is the OIDC group->scope map computed at login. Cloud: the
                    // caller's role in the CURRENT workspace (`session.tenant_id`),
                    // so switching workspaces re-derives scopes from membership and
                    // never trusts a scope set minted for a different tenant. A
                    // cloud session whose user has no membership in the current
                    // tenant is treated as insufficient (403), never authorized.
                    let effective_scopes = if st.multi_tenant {
                        match st
                            .store
                            .get_membership(session.user_id, session.tenant_id)
                            .await
                        {
                            Ok(Some(m)) => crate::tenant::role_scopes(m.role).to_vec(),
                            // No membership in the current tenant -> empty scopes.
                            // The covering check below fails and the unconditional
                            // `saw_insufficient = true` after it yields 403; setting
                            // the flag here too would be a dead assignment.
                            Ok(None) => vec![],
                            Err(_) => {
                                saw_store_error = true;
                                vec![]
                            }
                        }
                    } else {
                        session.scopes.clone()
                    };
                    if effective_scopes.iter().any(|s| s.covers(required)) {
                        return Ok(Principal {
                            tenant: session.tenant_id,
                            user_id: Some(session.user_id),
                            scopes: effective_scopes,
                        });
                    }
                    saw_insufficient = true;
                }
                Ok(None) => {}
                Err(_) => saw_store_error = true,
            }
        }
    }

    if saw_store_error {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    if saw_rate_limited {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    if saw_insufficient {
        return Err(StatusCode::FORBIDDEN);
    }
    Err(not_found_status)
}

/// Cookie-authenticated state-changing requests must carry a custom header the
/// browser cannot attach to a cross-site "simple" request. A request bearing
/// `x-admin-token` or `x-quark-csrf` is either token-authenticated (not
/// cookie-borne, so not CSRF-able) or was preflighted (custom header forces
/// preflight, gated by the CORS allowlist); either proves it is not a forgeable
/// simple POST riding the SameSite=None session cookie. Call after `admin_guard`
/// on simple-POST endpoints (no JSON body / content-type-sniffed body) that
/// would otherwise be reachable cross-site.
fn csrf_guard(headers: &HeaderMap) -> Result<(), StatusCode> {
    if headers.contains_key("x-admin-token") || headers.contains_key("x-quark-csrf") {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

/// Name of the session cookie set after a successful OIDC login.
const SESSION_COOKIE: &str = "qk_session";
/// Name of the short-lived cookie carrying the login-attempt state (PKCE
/// verifier + state + nonce) from `/admin/login` to `/admin/callback`.
const LOGIN_COOKIE: &str = "qk_login";
/// How long a login session lasts.
const SESSION_TTL_SECS: u64 = 12 * 3600;

#[derive(Deserialize)]
struct LoginParams {
    /// Tenant slug for a per-tenant login (multi-tenancy P2d,
    /// `/admin/login?org=<slug>`). Absent (or empty) means the global/OSS
    /// login against the env-configured IdP, unchanged.
    org: Option<String>,
}

/// The single, generic 404 for every `?org=<slug>` login failure that must
/// not leak whether the slug is real (multi-tenancy P2d Task 4b): an unknown
/// slug and a real tenant with no OIDC config of its own are otherwise
/// distinguishable messages, which would let an unauthenticated caller
/// enumerate valid organization slugs one probe at a time.
fn org_login_not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        "organization not found or has no identity provider",
    )
        .into_response()
}

/// `GET /admin/login`: start the OIDC Authorization Code + PKCE flow, stashing
/// the state/verifier/nonce in a short-lived signed cookie and redirecting to
/// the IdP.
///
/// With `?org=<slug>` (multi-tenancy P2d, cloud-only) this resolves the
/// tenant by slug and uses *its own* IdP config instead of the global one,
/// signing the tenant id into the cookie so the callback validates against
/// that same tenant. An unknown slug or a tenant with no OIDC config of its
/// own is an explicit error here — it never falls through to the global IdP,
/// which would let a caller land on the wrong tenant's login/identity.
async fn oidc_login(
    State(st): State<Arc<AppState>>,
    Query(params): Query<LoginParams>,
    headers: HeaderMap,
) -> Response {
    let (runtime, tenant) = match params.org.as_deref().filter(|s| !s.is_empty()) {
        Some(slug) => {
            if !st.multi_tenant {
                return (
                    StatusCode::NOT_FOUND,
                    "organization login requires a cloud deployment",
                )
                    .into_response();
            }
            // Rate-limit the org-login start by IP (LUC-51): the generic 404
            // already hides slug existence by body, but an unthrottled caller
            // could still probe slugs by response timing (unknown = 1 query vs
            // configured = more). A per-IP brake closes that side channel.
            let ip = client_ip(&headers, &st.real_ip_header, None);
            if !st.ratelimiter.check(&ip, now()).await {
                return (StatusCode::TOO_MANY_REQUESTS, "too many requests").into_response();
            }
            // Both "unknown slug" and "known tenant, no IdP of its own" return
            // the exact same 404 body: an unauthenticated caller must not be
            // able to distinguish a nonexistent organization from a real one
            // that simply hasn't set up OIDC (slug enumeration).
            let tenant = match st.store.get_tenant_by_slug(slug).await {
                Ok(Some(t)) => t,
                Ok(None) => return org_login_not_found(),
                Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
            };
            let cfg = match st.store.get_oidc_config_bare(tenant.id).await {
                Ok(Some(c)) => c,
                Ok(None) => return org_login_not_found(),
                Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
            };
            let rt = match st.oidc_tenants.get_or_build(tenant.id, &cfg).await {
                Ok(rt) => rt,
                Err(_) => {
                    return (
                        StatusCode::BAD_GATEWAY,
                        "organization's identity provider is unreachable",
                    )
                        .into_response()
                }
            };
            (rt, Some(tenant.id))
        }
        None => {
            let Some(oidc) = st.oidc.as_ref() else {
                return (StatusCode::NOT_FOUND, "oidc not configured").into_response();
            };
            (oidc.clone(), None)
        }
    };
    let state = crate::oidc::random_token();
    let nonce = crate::oidc::random_token();
    let (verifier, challenge) = crate::oidc::pkce_pair();
    let url = runtime.authorize_url(&state, &nonce, &challenge);
    let value = crate::oidc::sign_login_state(&st.signing_key, &state, &verifier, &nonce, tenant);
    let secure = if request_is_https(&headers) {
        "; Secure"
    } else {
        ""
    };
    // Path=/ (not /admin) so the cookie is sent to the callback regardless of the
    // configured QUARK_OIDC_REDIRECT_URL path.
    let cookie =
        format!("{LOGIN_COOKIE}={value}; Max-Age=600; Path=/; HttpOnly; SameSite=Lax{secure}");
    (
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, url),
            (header::SET_COOKIE, cookie),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
    )
        .into_response()
}

#[derive(Deserialize)]
struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// `GET /admin/callback`: verify the login-state cookie and `state`, exchange
/// the code (with the PKCE verifier), verify the id_token, map claims to scopes
/// (default-closed), and create a session. A valid IdP user with no granted
/// scope gets `403` (authenticated but unauthorized).
///
/// The tenant (if any) that validates this callback comes *only* from the
/// HMAC-signed `qk_login` cookie minted at `/admin/login` — never from a
/// client-supplied parameter on this request — so a tampered or forged
/// tenant cannot redirect validation to a different IdP than the one the
/// login actually started against.
async fn oidc_callback(
    State(st): State<Arc<AppState>>,
    Query(params): Query<CallbackParams>,
    headers: HeaderMap,
) -> Response {
    // Restore the pre-multi-tenancy status contract: when this callback
    // cannot resolve a tenant from the login cookie (no cookie, or one that
    // fails to verify) — i.e. it can only be the global env-IdP path — and
    // the global OIDC is not configured at all, this is `404` ("oidc not
    // configured"), exactly as before per-tenant login existed, and BEFORE
    // any other check (`?error=`, cookie presence, `state`, `code`). A
    // tenant carried by a cookie that verifies successfully takes the
    // per-tenant path below, unaffected by this check.
    let login = cookie_value(&headers, LOGIN_COOKIE)
        .and_then(|c| crate::oidc::verify_login_state(&st.signing_key, c));
    let tenant_from_cookie = login.as_ref().and_then(|(_, _, _, t)| *t);
    if tenant_from_cookie.is_none() && (st.oidc.is_none() || !st.oidc_configured) {
        return (StatusCode::NOT_FOUND, "oidc not configured").into_response();
    }

    if params.error.is_some() {
        return (StatusCode::UNAUTHORIZED, "login was denied at the provider").into_response();
    }
    let Some((state, verifier, nonce, tenant)) = login else {
        return (StatusCode::BAD_REQUEST, "missing or invalid login state").into_response();
    };
    // CSRF: the state echoed by the IdP must match the one we signed.
    if params.state.as_deref() != Some(state.as_str()) {
        return (StatusCode::BAD_REQUEST, "state mismatch").into_response();
    }
    let Some(code) = params.code else {
        return (StatusCode::BAD_REQUEST, "missing code").into_response();
    };

    // Resolve which IdP runtime and config validate this callback: the
    // tenant signed into the login cookie (multi-tenancy P2d), or the global
    // env-configured IdP.
    let (runtime, tenant_cfg) = match tenant {
        Some(tenant_id) => {
            let cfg = match st.store.get_oidc_config_bare(tenant_id).await {
                Ok(Some(c)) => c,
                Ok(None) => {
                    // The tenant's config was removed after the login started.
                    return (
                        StatusCode::BAD_REQUEST,
                        "organization's identity provider is no longer configured",
                    )
                        .into_response();
                }
                Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
            };
            let rt = match st.oidc_tenants.get_or_build(tenant_id, &cfg).await {
                Ok(rt) => rt,
                Err(_) => {
                    return (
                        StatusCode::BAD_GATEWAY,
                        "organization's identity provider is unreachable",
                    )
                        .into_response()
                }
            };
            (rt, Some(cfg))
        }
        None => {
            let Some(oidc) = st.oidc.as_ref() else {
                return (StatusCode::NOT_FOUND, "oidc not configured").into_response();
            };
            (oidc.clone(), None)
        }
    };

    let id_token = match runtime.exchange_code(&code, &verifier).await {
        Ok(t) => t,
        Err(_) => return (StatusCode::BAD_GATEWAY, "token exchange failed").into_response(),
    };
    let claims = match runtime.verify(&id_token, &nonce).await {
        Ok(c) => c,
        Err(_) => return (StatusCode::UNAUTHORIZED, "invalid id_token").into_response(),
    };
    let email = claims
        .raw
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Per-tenant login (multi-tenancy P2d): the role comes from the tenant's
    // own claim mapping and always grants at least Member (see `claim_role`).
    // Global/OSS login: unchanged, default-closed scope mapping; no granted
    // scope is a 403.
    let (scopes, tenant_membership, session_tenant, dest) = match (tenant, &tenant_cfg) {
        (Some(tenant_id), Some(cfg)) => {
            // Required-group gate (multi-tenancy P2d Task 4b), checked BEFORE
            // computing/granting anything: when the tenant configured a
            // `required_value`, a claim matching neither `admin_value`,
            // `readonly_value`, nor `required_value` is denied outright — no
            // membership, no session. When unset, every authenticated tenant
            // IdP user is admitted, same as before this gate existed.
            if !crate::oidc::passes_required_group(&claims.raw, cfg) {
                return (
                    StatusCode::FORBIDDEN,
                    "your account is not in the required group for this organization",
                )
                    .into_response();
            }
            let role = crate::oidc::claim_role(&claims.raw, cfg);
            (
                crate::tenant::role_scopes(role).to_vec(),
                Some((tenant_id, role)),
                tenant_id,
                cfg.post_login_url
                    .clone()
                    .unwrap_or_else(|| "/".to_string()),
            )
        }
        _ => {
            let scopes = crate::oidc::map_scopes(&claims.raw, &runtime.config);
            if scopes.is_empty() {
                return (StatusCode::FORBIDDEN, "your account has no quark access").into_response();
            }
            (
                scopes,
                None,
                crate::tenant::DEFAULT_TENANT,
                runtime.config.post_login_url.clone(),
            )
        }
    };

    let user_id = match crate::oidc::ensure_user_and_membership(
        st.store.as_ref(),
        st.multi_tenant,
        &claims.subject,
        &email,
        &claims.display,
        &scopes,
        tenant_membership,
    )
    .await
    {
        Ok(id) => id,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let raw = generate_token();
    let now = now();
    let session = crate::auth::Session {
        token_hash: hash_token(&raw),
        subject: claims.subject,
        display: claims.display,
        scopes,
        created: now,
        expires: now + SESSION_TTL_SECS,
        tenant_id: session_tenant,
        user_id,
    };
    if st
        .store
        .put_session(session_tenant, &session)
        .await
        .is_err()
    {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    // Over HTTPS use SameSite=None; Secure so the cookie is sent on cross-origin
    // panel->API fetches (the documented split-origin deployment). On plain HTTP
    // (local dev, same-origin) SameSite=None is invalid without Secure, so fall
    // back to Lax.
    let same_site = if request_is_https(&headers) {
        "None; Secure"
    } else {
        "Lax"
    };
    let cookie = format!(
        "{SESSION_COOKIE}={raw}; Max-Age={SESSION_TTL_SECS}; Path=/; HttpOnly; SameSite={same_site}"
    );
    // Redirect to the configured post-login URL (the panel), default "/".
    // Clear the now-consumed login-state cookie so it cannot be replayed.
    let clear_login = format!("{LOGIN_COOKIE}=; Max-Age=0; Path=/; HttpOnly; SameSite=Lax");
    let mut resp = (
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, dest),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
    )
        .into_response();
    // Append both Set-Cookie headers (an array of tuples would overwrite one).
    let h = resp.headers_mut();
    if let Ok(v) = cookie.parse() {
        h.append(header::SET_COOKIE, v);
    }
    if let Ok(v) = clear_login.parse() {
        h.append(header::SET_COOKIE, v);
    }
    resp
}

/// `POST /admin/logout`: revoke the current session and clear the cookie.
/// Requires the `x-quark-csrf` header the panel sends: without it a cross-site
/// simple POST could force-logout via the SameSite=None cookie, and with it the
/// request is preflighted so the CORS allowlist gates any cross-origin caller.
async fn oidc_logout(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if headers.get("x-quark-csrf").is_none() {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Some(raw) = cookie_value(&headers, SESSION_COOKIE) {
        let _ = st.store.delete_session(&hash_token(raw)).await;
    }
    let clear = format!("{SESSION_COOKIE}=; Max-Age=0; Path=/; HttpOnly; SameSite=Lax");
    (
        StatusCode::NO_CONTENT,
        [
            (header::SET_COOKIE, clear),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
    )
        .into_response()
}

/// `GET /admin/me`: the current principal (from the session cookie) plus whether
/// OIDC is configured, so the panel can render the login button and signed-in
/// state. Never guarded: it reports `authenticated: false` instead of erroring.
async fn admin_me(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let oidc_enabled = st.oidc.is_some();
    if let Some(raw) = cookie_value(&headers, SESSION_COOKIE) {
        if let Ok(Some(session)) = st.store.get_session_by_hash(&hash_token(raw), now()).await {
            let memberships = if st.multi_tenant {
                let ms = st
                    .store
                    .list_memberships_for_user(session.user_id)
                    .await
                    .unwrap_or_default();
                let mut out = Vec::new();
                for m in ms {
                    if let Ok(Some(t)) = st.store.get_tenant(m.tenant_id).await {
                        out.push(serde_json::json!({
                            "tenant_id": t.id.0,
                            "name": t.name,
                            "slug": t.slug,
                            "role": m.role,
                        }));
                    }
                }
                out
            } else {
                Vec::new()
            };
            // The current workspace is the session's tenant ONLY when the user
            // actually has a membership there. A fresh cloud user's session still
            // carries DEFAULT_TENANT (0) with no membership in it, so report
            // `null` to signal onboarding rather than a phantom "workspace 0".
            let current_tenant = if st.multi_tenant {
                match st
                    .store
                    .get_membership(session.user_id, session.tenant_id)
                    .await
                {
                    Ok(Some(_)) => Some(session.tenant_id.0),
                    _ => None,
                }
            } else {
                None
            };
            return Json(serde_json::json!({
                "authenticated": true,
                "display": session.display,
                "scopes": session.scopes,
                "oidc_enabled": oidc_enabled,
                "multi_tenant": st.multi_tenant,
                "memberships": memberships,
                "current_tenant": current_tenant,
                "tenant_domain_suffix": st.tenant_domain_suffix,
            }))
            .into_response();
        }
    }
    Json(serde_json::json!({
        "authenticated": false,
        "oidc_enabled": oidc_enabled,
        "multi_tenant": st.multi_tenant,
        "tenant_domain_suffix": st.tenant_domain_suffix,
    }))
    .into_response()
}

/// Resolves the session cookie to its `user_id`, independent of scopes. Used
/// by `/admin/tenants`: creating a first workspace must be reachable by ANY
/// authenticated OIDC user, including one with zero memberships (so
/// `admin_guard`'s scope check, which a 0-membership user could never pass,
/// does not apply here). Gated on `st.oidc_configured`, same as `admin_guard`'s
/// session branch, so disabling OIDC immediately stops leftover session
/// cookies from resolving a user here too.
async fn session_user_id(st: &AppState, headers: &HeaderMap) -> Option<u64> {
    if !st.oidc_configured {
        return None;
    }
    let raw = cookie_value(headers, SESSION_COOKIE)?;
    let session = st
        .store
        .get_session_by_hash(&hash_token(raw), now())
        .await
        .ok()
        .flatten()?;
    Some(session.user_id)
}

/// Re-points the current session at `tenant` (the workspace just created),
/// so the next request the browser makes is already scoped to it. A missing
/// or invalid session is a silent no-op: the caller already authenticated via
/// `session_user_id` earlier in the same request.
async fn set_session_tenant(st: &AppState, headers: &HeaderMap, tenant: crate::tenant::TenantId) {
    let Some(raw) = cookie_value(headers, SESSION_COOKIE) else {
        return;
    };
    let hash = hash_token(raw);
    if let Ok(Some(mut session)) = st.store.get_session_by_hash(&hash, now()).await {
        session.tenant_id = tenant;
        let _ = st.store.put_session(tenant, &session).await;
    }
}

/// Maps a `put_tenant` failure to its HTTP status: a duplicate `slug` (unique
/// violation) is a client-fixable `409`, anything else is a `503` (backend
/// unavailable).
fn conflict_or_503(e: StoreError) -> StatusCode {
    match e {
        StoreError::UniqueViolation => StatusCode::CONFLICT,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    }
}

/// The host of a tenant's automatic subdomain (multi-tenancy P3-completion),
/// e.g. `subdomain_host("acme", "quarkus.com.br") == "acme.quarkus.com.br"`.
/// Lowercased so it matches the lookup convention every other host in
/// `domains` follows (`get_domain_by_host`/`HostRouter` always query lowercase).
pub fn subdomain_host(slug: &str, suffix: &str) -> String {
    format!("{slug}.{suffix}").to_ascii_lowercase()
}

/// Ensures the tenant's automatic subdomain exists as a Verified `domains`
/// row (multi-tenancy P3-completion). Called both on tenant creation and by
/// the boot-time backfill (`main.rs`) for pre-existing tenants.
///
/// Deliberately blind-inserts via `next_domain_id()` + `put_domain` rather
/// than checking `get_domain_by_host` first: the id comes from the same
/// sequence real custom domains use, so a fresh id never collides, and the
/// `host` UNIQUE constraint is the actual idempotency guard — a second seed
/// attempt hits `StoreError::UniqueViolation`, which is treated as success
/// (the row already exists, nothing left to do). Subdomains aren't
/// DNS-verified (there's no TXT record to check — the app itself owns
/// `*.<suffix>`), so `token` is empty and `status` starts `Verified`.
pub async fn seed_tenant_subdomain(
    store: &Arc<dyn Store>,
    tenant_id: crate::tenant::TenantId,
    slug: &str,
    suffix: &str,
) -> Result<(), StoreError> {
    let id = store.next_domain_id().await?;
    let ts = now();
    let domain = Domain {
        id,
        tenant_id,
        host: subdomain_host(slug, suffix),
        token: String::new(),
        status: DomainStatus::Verified,
        created: ts,
        verified_at: Some(ts),
    };
    match store.put_domain(&domain).await {
        Ok(()) | Err(StoreError::UniqueViolation) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Logs one failed provisioning step as a single-line JSON error, the same
/// shape `admin_tenants_create`'s subdomain seed already uses for its own
/// best-effort failures.
fn log_keycloak_step_error(tenant_id: u64, step: &str, err: impl std::fmt::Display) {
    eprintln!(
        "{}",
        serde_json::json!({
            "keycloak_provision_error": err.to_string(),
            "step": step,
            "tenant_id": tenant_id,
        })
    );
}

/// Runs the full per-tenant Keycloak provisioning sequence (multi-tenancy
/// P2e): realm, then client, then groups/mapper, then the owner user (in
/// `quark-admins`) and their set-password email, then the tenant's
/// `oidc_config`. Every step is best-effort: a failure is logged and
/// provisioning stops at that step (the caller — `admin_tenants_create` or
/// the boot backfill — never fails because of it), and every `KeycloakAdmin`
/// method is idempotent (`409` = success in the real client), so calling
/// this again on an already-provisioned tenant is a safe no-op, which is
/// exactly how the boot backfill retries a tenant whose earlier attempt only
/// got partway.
///
/// `owner_user_id` is `Some` when the caller (`admin_tenants_create`) knows
/// who just became Owner — their email drives `ensure_user`. It is `None`
/// for the boot backfill, which has no way to look up "the tenant's Owner"
/// outside of that request context; in that case the admin-user step (and its
/// set-password email) is skipped, but the realm/client/groups/`oidc_config`
/// still get provisioned. The same skip happens when the owner's `User` row
/// has no email on file.
pub async fn provision_tenant_keycloak(
    store: &Arc<dyn Store>,
    kc: &dyn crate::keycloak::KeycloakAdmin,
    base_url: &str,
    tenant: &crate::tenant::Tenant,
    owner_user_id: Option<u64>,
) {
    let redirect_uri = std::env::var("QUARK_OIDC_REDIRECT_URL").unwrap_or_default();
    if let Err(e) = kc.ensure_realm(&tenant.slug).await {
        log_keycloak_step_error(tenant.id.0, "ensure_realm", e);
        return;
    }
    if let Err(e) = kc.ensure_client(&tenant.slug, &redirect_uri).await {
        log_keycloak_step_error(tenant.id.0, "ensure_client", e);
        return;
    }
    if let Err(e) = kc.ensure_groups_and_mapper(&tenant.slug).await {
        log_keycloak_step_error(tenant.id.0, "ensure_groups_and_mapper", e);
        return;
    }
    if let Some(uid) = owner_user_id {
        match store.get_user_by_id(uid).await {
            Ok(Some(u)) if !u.email.is_empty() => {
                match kc.ensure_user(&tenant.slug, &u.email, "quark-admins").await {
                    Ok(kc_user_id) => {
                        if let Err(e) = kc.send_set_password_email(&tenant.slug, &kc_user_id).await
                        {
                            log_keycloak_step_error(tenant.id.0, "send_set_password_email", e);
                        }
                    }
                    Err(e) => log_keycloak_step_error(tenant.id.0, "ensure_user", e),
                }
            }
            Ok(_) => eprintln!(
                "{}",
                serde_json::json!({
                    "keycloak_provision_skip": "owner email unavailable",
                    "tenant_id": tenant.id.0,
                })
            ),
            Err(e) => log_keycloak_step_error(tenant.id.0, "get_user_by_id", e),
        }
    }
    // Public client + PKCE (see `HttpKeycloakAdmin::ensure_client`): no
    // client secret exists for quark to hold, so this is always empty.
    let cfg = crate::oidc::TenantOidcConfig {
        tenant_id: tenant.id,
        issuer: crate::keycloak::derive_issuer(base_url, &tenant.slug),
        client_id: "quark".to_string(),
        client_secret: String::new(),
        scopes: vec![
            "openid".to_string(),
            "profile".to_string(),
            "email".to_string(),
        ],
        admin_claim: "groups".to_string(),
        admin_value: "quark-admins".to_string(),
        readonly_value: "quark-readers".to_string(),
        // Default-closed: only quark-admins/quark-readers members are
        // admitted (see `oidc::passes_required_group`), never the open
        // `Role::Member` fallback `claim_role` would otherwise grant to any
        // authenticated realm user.
        required_value: Some("quark-readers".to_string()),
        post_login_url: None,
    };
    if let Err(e) = store.put_oidc_config(&cfg).await {
        log_keycloak_step_error(tenant.id.0, "put_oidc_config", e);
    }
}

/// Boot backfill (multi-tenancy P2e): provisions Keycloak for every tenant
/// that has no `oidc_config` yet, via `provision_tenant_keycloak` — the same
/// steps `admin_tenants_create` runs, so a tenant whose creation-time attempt
/// only got partway is retried to completion here. Returns how many tenants
/// were (re-)provisioned this pass. Mirrors the shape of the per-tenant
/// subdomain backfill next to it in `main.rs`.
pub async fn backfill_keycloak_provisioning(
    store: &Arc<dyn Store>,
    keycloak: &Arc<dyn crate::keycloak::KeycloakAdmin>,
    base_url: &str,
) -> Result<usize, StoreError> {
    let tenants = store.list_tenants().await?;
    let mut provisioned = 0usize;
    for t in &tenants {
        match store.get_oidc_config_bare(t.id).await {
            Ok(Some(_)) => {} // already provisioned
            Ok(None) => {
                provision_tenant_keycloak(store, keycloak.as_ref(), base_url, t, None).await;
                provisioned += 1;
            }
            Err(e) => eprintln!(
                "{}",
                serde_json::json!({ "keycloak_backfill_error": e.to_string(), "tenant_id": t.id.0 })
            ),
        }
    }
    Ok(provisioned)
}

#[derive(Deserialize)]
struct CreateTenantReq {
    name: String,
    slug: String,
}

/// `POST /admin/tenants`: self-serve workspace creation (cloud only). Any
/// authenticated OIDC user may create a workspace — not gated by
/// `admin_guard`'s scope check, since a user with zero memberships must still
/// be able to create their first one. Creates the `Tenant`, grants the
/// caller `Owner` on it, and re-points their session at it.
async fn admin_tenants_create(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateTenantReq>,
) -> Response {
    if !st.multi_tenant {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(user_id) = session_user_id(&st, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let ip = client_ip(&headers, &st.real_ip_header, None);
    if !st.ratelimiter.check(&ip, now()).await {
        return (StatusCode::TOO_MANY_REQUESTS, "too many requests").into_response();
    }
    // Reject before any side effect: P2e turns the slug verbatim into a
    // Keycloak realm name, an Admin-API URL path, and the derived OIDC
    // issuer, so a bad slug must never reach the store or Keycloak.
    if !crate::tenant::is_valid_slug(&req.slug) {
        return (StatusCode::BAD_REQUEST, "invalid slug").into_response();
    }
    let id = match st.store.next_tenant_id().await {
        Ok(i) => i,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let tenant = crate::tenant::Tenant {
        id: crate::tenant::TenantId(id),
        name: req.name,
        slug: req.slug,
        created: now(),
    };
    if let Err(e) = st.store.put_tenant(&tenant).await {
        return conflict_or_503(e).into_response();
    }
    if st
        .store
        .put_membership(&crate::tenant::Membership {
            user_id,
            tenant_id: crate::tenant::TenantId(id),
            role: crate::tenant::Role::Owner,
            created: now(),
        })
        .await
        .is_err()
    {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    // Auto per-tenant subdomain (multi-tenancy P3-completion): best-effort —
    // a failure here must not fail the tenant creation itself, since the
    // tenant and its Owner membership are already committed. The panel falls
    // back to the shared host until this is retried (boot backfill covers it).
    if let Some(suffix) = &st.tenant_domain_suffix {
        match seed_tenant_subdomain(&st.store, tenant.id, &tenant.slug, suffix).await {
            Ok(()) => {
                st.host_router
                    .invalidate(&subdomain_host(&tenant.slug, suffix))
                    .await
            }
            Err(e) => eprintln!(
                "{}",
                serde_json::json!({ "tenant_subdomain_seed_error": e.to_string(), "tenant_id": id })
            ),
        }
    }
    // Keycloak realm auto-provisioning (multi-tenancy P2e): same best-effort
    // shape as the subdomain seed above — the tenant and its Owner membership
    // are already committed, so a provisioning failure here must not fail
    // this 201 (the boot backfill retries it).
    if let Some(kc) = &st.keycloak {
        provision_tenant_keycloak(
            &st.store,
            kc.as_ref(),
            st.keycloak_base_url.as_deref().unwrap_or_default(),
            &tenant,
            Some(user_id),
        )
        .await;
    }
    set_session_tenant(&st, &headers, crate::tenant::TenantId(id)).await;
    Json(tenant).into_response()
}

#[derive(Deserialize)]
struct SwitchReq {
    tenant_id: u64,
}

/// `POST /admin/workspace/switch`: change the session's current workspace
/// (cloud only). SECURITY: always validates membership before switching — a
/// caller may only switch to a tenant they belong to. A missing membership
/// leaves the session untouched and returns `403`, rather than mutating it
/// and failing closed some other way.
async fn admin_workspace_switch(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<SwitchReq>,
) -> Response {
    if !st.multi_tenant {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(user_id) = session_user_id(&st, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match st
        .store
        .get_membership(user_id, crate::tenant::TenantId(req.tenant_id))
        .await
    {
        Ok(Some(_)) => {
            set_session_tenant(&st, &headers, crate::tenant::TenantId(req.tenant_id)).await;
            StatusCode::OK.into_response()
        }
        Ok(None) => StatusCode::FORBIDDEN.into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

// --- Custom domains (multi-tenancy P3), cloud-only ---

/// Name of the TXT record a caller must publish to prove ownership of a
/// custom domain.
fn verify_txt_name(host: &str) -> String {
    format!("_quark-verify.{host}")
}

/// A minimal syntax check for a host a caller wants to bind: dotted labels,
/// each 1-63 characters of alphanumerics/hyphens, no leading/trailing hyphen.
/// Not a full RFC 1035 validator, just enough to reject obvious junk before
/// it reaches the store.
fn is_valid_host_format(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 || !host.contains('.') {
        return false;
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    })
}

#[derive(Deserialize)]
struct CreateDomainReq {
    host: String,
}

/// A domain plus the DNS instructions to verify it: the panel and any caller
/// need both together, so `list`/`create`/`verify` all return this shape
/// rather than the bare store `Domain`.
#[derive(Serialize)]
struct DomainView {
    id: u64,
    host: String,
    status: DomainStatus,
    created: u64,
    verified_at: Option<u64>,
    /// Name of the TXT record to publish: `_quark-verify.<host>`.
    txt_name: String,
    /// Value the TXT record must hold (the domain's verification token).
    txt_value: String,
    /// CNAME target the caller should point `host` at. `None` when this
    /// deploy has no shared `public_host` configured.
    cname_target: Option<String>,
}

fn domain_view(d: &Domain, public_host: &Option<String>) -> DomainView {
    DomainView {
        id: d.id,
        host: d.host.clone(),
        status: d.status,
        created: d.created,
        verified_at: d.verified_at,
        txt_name: verify_txt_name(&d.host),
        txt_value: d.token.clone(),
        cname_target: public_host.clone(),
    }
}

/// `GET /admin/domains`: list the caller's tenant's custom domains, each with
/// the DNS instructions needed to verify it (cloud only).
async fn admin_domains_list(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !st.multi_tenant {
        return StatusCode::NOT_FOUND.into_response();
    }
    let p = match admin_guard(&st, &headers, Scope::Full).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    match st.store.list_domains(p.tenant).await {
        Ok(domains) => Json(
            domains
                .iter()
                .map(|d| domain_view(d, &st.public_host))
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// `POST /admin/domains {host}`: register a custom domain for the caller's
/// tenant, pending DNS verification (cloud only). Rejects internal hosts and
/// the shared `public_host`, and normalizes `host` (lowercase, trimmed, no
/// trailing dot) before storing — `get_domain_by_host`/`HostRouter` always
/// query lowercase, so an un-normalized host would never resolve. Duplicate
/// `host` (unique violation) -> 409.
async fn admin_domains_create(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateDomainReq>,
) -> Response {
    if !st.multi_tenant {
        return StatusCode::NOT_FOUND.into_response();
    }
    let p = match admin_guard(&st, &headers, Scope::Full).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    let ip = client_ip(&headers, &st.real_ip_header, None);
    if !st.ratelimiter.check(&ip, now()).await {
        return (StatusCode::TOO_MANY_REQUESTS, "too many requests").into_response();
    }
    let host = req.host.trim().trim_end_matches('.').to_ascii_lowercase();
    if !is_valid_host_format(&host) {
        return (StatusCode::BAD_REQUEST, "invalid host").into_response();
    }
    if is_internal_host(&host) || st.public_host.as_deref() == Some(host.as_str()) {
        return (StatusCode::BAD_REQUEST, "host not allowed").into_response();
    }
    let id = match st.store.next_domain_id().await {
        Ok(i) => i,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let domain = Domain {
        id,
        tenant_id: p.tenant,
        host,
        token: generate_token(),
        status: DomainStatus::Pending,
        created: now(),
        verified_at: None,
    };
    if let Err(e) = st.store.put_domain(&domain).await {
        return conflict_or_503(e).into_response();
    }
    Json(domain_view(&domain, &st.public_host)).into_response()
}

/// `POST /admin/domains/:id/verify`: look up the `_quark-verify.<host>` TXT
/// record for the caller's tenant's domain; on a match, mark it `Verified`
/// and invalidate the host router so the new route takes effect immediately.
/// A missing or mismatched TXT record leaves the domain `Pending`.
async fn admin_domains_verify(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<u64>,
) -> Response {
    if !st.multi_tenant {
        return StatusCode::NOT_FOUND.into_response();
    }
    let p = match admin_guard(&st, &headers, Scope::Full).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    let ip = client_ip(&headers, &st.real_ip_header, None);
    if !st.ratelimiter.check(&ip, now()).await {
        return (StatusCode::TOO_MANY_REQUESTS, "too many requests").into_response();
    }
    let mut domain = match st.store.get_domain(p.tenant, id).await {
        Ok(Some(d)) => d,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let txt_name = verify_txt_name(&domain.host);
    let matched = st
        .dns
        .lookup_txt(&txt_name)
        .await
        .map(|values| values.iter().any(|v| v == &domain.token))
        .unwrap_or(false);
    if matched {
        let verified_at = now();
        if st
            .store
            .set_domain_status(p.tenant, id, DomainStatus::Verified, Some(verified_at))
            .await
            .is_err()
        {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        st.host_router.invalidate(&domain.host).await;
        domain.status = DomainStatus::Verified;
        domain.verified_at = Some(verified_at);
    }
    Json(domain_view(&domain, &st.public_host)).into_response()
}

/// `DELETE /admin/domains/:id`: remove the caller's tenant's custom domain
/// and drop any cached host-router entry for it (cloud only).
async fn admin_domains_delete(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<u64>,
) -> Response {
    if !st.multi_tenant {
        return StatusCode::NOT_FOUND.into_response();
    }
    let p = match admin_guard(&st, &headers, Scope::Full).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    let domain = match st.store.get_domain(p.tenant, id).await {
        Ok(Some(d)) => d,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    if st.store.delete_domain(p.tenant, id).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    st.host_router.invalidate(&domain.host).await;
    StatusCode::NO_CONTENT.into_response()
}

// --- SSO email-domain discovery (LUC-57 Task 2), cloud-only ---

/// Name of the TXT record a caller must publish to prove ownership of an
/// email domain for SSO discovery (mirrors `verify_txt_name`, but under its
/// own label so it never collides with the P3 custom-domain record).
fn sso_verify_txt_name(domain: &str) -> String {
    format!("_quark-sso.{domain}")
}

/// An SSO email domain plus the DNS instructions to verify it (mirrors
/// `DomainView`).
#[derive(Serialize)]
struct SsoDomainView {
    id: u64,
    domain: String,
    status: DomainStatus,
    created: u64,
    verified_at: Option<u64>,
    /// Name of the TXT record to publish: `_quark-sso.<domain>`.
    txt_name: String,
    /// Value the TXT record must hold (the domain's verification token).
    txt_value: String,
}

fn sso_domain_view(d: &SsoEmailDomain) -> SsoDomainView {
    SsoDomainView {
        id: d.id,
        domain: d.domain.clone(),
        status: d.status,
        created: d.created,
        verified_at: d.verified_at,
        txt_name: sso_verify_txt_name(&d.domain),
        txt_value: d.token.clone(),
    }
}

#[derive(Deserialize)]
struct CreateSsoDomainReq {
    domain: String,
}

/// `GET /admin/sso-domains`: list the caller's tenant's SSO email domains,
/// each with the DNS instructions needed to verify it (cloud only).
async fn admin_sso_domains_list(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !st.multi_tenant {
        return StatusCode::NOT_FOUND.into_response();
    }
    let p = match admin_guard(&st, &headers, Scope::Full).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    match st.store.list_sso_domains(p.tenant).await {
        Ok(domains) => {
            Json(domains.iter().map(sso_domain_view).collect::<Vec<_>>()).into_response()
        }
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// `POST /admin/sso-domains {domain}`: registers an email domain for SSO
/// discovery under the caller's tenant, pending DNS verification (cloud
/// only). Requires the tenant to already have an `oidc_config`: there is no
/// point discovering a domain into a tenant with nowhere to route the login
/// -- that gate returns `409` ("SSO not configured"). Normalizes `domain`
/// (lowercase, trimmed) and rejects anything that isn't a plausible domain
/// (reuses `normalize_email_domain` by prepending a dummy local part).
/// Duplicate `domain` (unique across tenants) -> `409`.
async fn admin_sso_domains_create(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateSsoDomainReq>,
) -> Response {
    if !st.multi_tenant {
        return StatusCode::NOT_FOUND.into_response();
    }
    let p = match admin_guard(&st, &headers, Scope::Full).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    let ip = client_ip(&headers, &st.real_ip_header, None);
    if !st.ratelimiter.check(&ip, now()).await {
        return (StatusCode::TOO_MANY_REQUESTS, "too many requests").into_response();
    }
    match st.store.get_oidc_config_bare(p.tenant).await {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::CONFLICT, "SSO not configured").into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    let raw = req.domain.trim().to_ascii_lowercase();
    let Some(domain) = normalize_email_domain(&format!("x@{raw}")) else {
        return (StatusCode::BAD_REQUEST, "invalid domain").into_response();
    };
    let id = match st.store.next_sso_domain_id().await {
        Ok(i) => i,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let sso_domain = SsoEmailDomain {
        id,
        tenant_id: p.tenant,
        domain,
        token: generate_token(),
        status: DomainStatus::Pending,
        created: now(),
        verified_at: None,
    };
    if let Err(e) = st.store.put_sso_domain(&sso_domain).await {
        return conflict_or_503(e).into_response();
    }
    Json(sso_domain_view(&sso_domain)).into_response()
}

/// `POST /admin/sso-domains/:id/verify`: looks up the `_quark-sso.<domain>`
/// TXT record for the caller's tenant's SSO email domain; on a match, marks
/// it `Verified`. A missing or mismatched TXT record leaves it `Pending`
/// (mirrors `admin_domains_verify`).
async fn admin_sso_domains_verify(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<u64>,
) -> Response {
    if !st.multi_tenant {
        return StatusCode::NOT_FOUND.into_response();
    }
    let p = match admin_guard(&st, &headers, Scope::Full).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    let mut sso_domain = match st.store.get_sso_domain(p.tenant, id).await {
        Ok(Some(d)) => d,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let txt_name = sso_verify_txt_name(&sso_domain.domain);
    let matched = st
        .dns
        .lookup_txt(&txt_name)
        .await
        .map(|values| values.iter().any(|v| v == &sso_domain.token))
        .unwrap_or(false);
    if matched {
        let verified_at = now();
        if st
            .store
            .set_sso_domain_status(p.tenant, id, DomainStatus::Verified, Some(verified_at))
            .await
            .is_err()
        {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        sso_domain.status = DomainStatus::Verified;
        sso_domain.verified_at = Some(verified_at);
    }
    Json(sso_domain_view(&sso_domain)).into_response()
}

/// `DELETE /admin/sso-domains/:id`: removes the caller's tenant's SSO email
/// domain (cloud only).
async fn admin_sso_domains_delete(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<u64>,
) -> Response {
    if !st.multi_tenant {
        return StatusCode::NOT_FOUND.into_response();
    }
    let p = match admin_guard(&st, &headers, Scope::Full).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    match st.store.get_sso_domain(p.tenant, id).await {
        Ok(Some(_)) => {}
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    if st.store.delete_sso_domain(p.tenant, id).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct DiscoverParams {
    email: Option<String>,
}

/// Response for `GET /admin/sso/discover`. Deliberately carries only the
/// tenant's slug (already public via `/admin/login?org=<slug>`) and never
/// `tenant_id`, and is shaped identically whether the email is malformed, the
/// domain is unknown, still pending, or its tenant lost its `oidc_config` --
/// an unauthenticated caller must not be able to tell those cases apart
/// (anti-enumeration; mirrors `org_login_not_found`'s uniform-404 intent, but
/// this endpoint is a lookup so it stays `200` throughout).
#[derive(Serialize, Default)]
struct DiscoverResp {
    #[serde(skip_serializing_if = "Option::is_none")]
    org: Option<String>,
}

/// `GET /admin/sso/discover?email=<email>`: Home Realm Discovery (cloud
/// only, PUBLIC -- no `admin_guard`). Given an email, resolves its domain to
/// a verified SSO email domain and returns the owning tenant's slug so the
/// login UI can send the user straight to `/admin/login?org=<slug>`.
///
/// Routes ONLY on a `Verified` domain whose tenant still has an
/// `oidc_config` and a resolvable slug; every other outcome (missing/
/// malformed email, unknown domain, still-`Pending`, or a tenant that lost
/// its `oidc_config`/no longer exists) returns the same empty `{}` -- never a
/// 404 or a distinguishable error, so this endpoint cannot be used to probe
/// which domains or tenants exist.
async fn sso_discover(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<DiscoverParams>,
) -> Response {
    if !st.multi_tenant {
        return StatusCode::NOT_FOUND.into_response();
    }
    let ip = client_ip(&headers, &st.real_ip_header, None);
    if !st.ratelimiter.check(&ip, now()).await {
        return (StatusCode::TOO_MANY_REQUESTS, "too many requests").into_response();
    }
    let empty = || Json(DiscoverResp::default()).into_response();
    let Some(email) = params.email else {
        return empty();
    };
    let Some(domain) = normalize_email_domain(&email) else {
        return empty();
    };
    let row = match st.store.get_sso_domain_bare(&domain).await {
        Ok(Some(r)) => r,
        Ok(None) => return empty(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    if row.status != DomainStatus::Verified {
        return empty();
    }
    match st.store.get_oidc_config_bare(row.tenant_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return empty(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    match st.store.get_tenant(row.tenant_id).await {
        Ok(Some(tenant)) => Json(DiscoverResp {
            org: Some(tenant.slug),
        })
        .into_response(),
        Ok(None) => empty(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

// --- Team invites (multi-tenancy P2c), cloud-only ---

/// How long a team invite stays valid before it must be re-sent.
const INVITE_TTL_SECS: u64 = 7 * 24 * 3600;

#[derive(Deserialize)]
struct CreateInviteReq {
    email: String,
    role: crate::tenant::Role,
}

#[derive(Serialize)]
struct CreateInviteResp {
    id: u64,
    token: String,
    email: String,
    role: crate::tenant::Role,
    expires: u64,
}

/// `POST /admin/invites {email, role}`: invite an email to join the caller's
/// tenant with `role` (cloud only, Owner/Admin required). Rejects `role:
/// "owner"` — an invite can never grant ownership, only Admin/Member/Viewer.
/// Returns the plaintext token once, same precedent as `admin_tokens_create`:
/// only the hash is persisted, so the caller must capture it from this
/// response (or the invite link it builds around it).
async fn admin_invites_create(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !st.multi_tenant {
        return StatusCode::NOT_FOUND.into_response();
    }
    let p = match admin_guard(&st, &headers, Scope::Full).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    let ip = client_ip(&headers, &st.real_ip_header, None);
    if !st.ratelimiter.check(&ip, now()).await {
        return (StatusCode::TOO_MANY_REQUESTS, "too many requests").into_response();
    }
    let req: CreateInviteReq = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid json").into_response(),
    };
    if req.role == crate::tenant::Role::Owner {
        return (
            StatusCode::BAD_REQUEST,
            "cannot invite a new member as owner",
        )
            .into_response();
    }
    let email = req.email.trim().to_ascii_lowercase();
    if email.is_empty() || !email.contains('@') {
        return (StatusCode::BAD_REQUEST, "invalid email").into_response();
    }
    let token = generate_token();
    let id = match st.store.next_invite_id().await {
        Ok(i) => i,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let created = now();
    let expires = created + INVITE_TTL_SECS;
    let invite = crate::invite::Invite {
        id,
        tenant_id: p.tenant,
        email: email.clone(),
        role: req.role,
        token_hash: hash_token(&token),
        // An API-token principal carries no `user_id` (`None`); `0` records
        // "invited by the tenant's token" rather than a real user.
        invited_by: p.user_id.unwrap_or(0),
        created,
        expires,
        accepted_at: None,
        accepted_by: None,
    };
    if let Err(e) = st.store.create_invite(&invite).await {
        return conflict_or_503(e).into_response();
    }
    // Keycloak realm provisioning (multi-tenancy P2e Task 3): best-effort,
    // same shape as `provision_tenant_keycloak` — the invite row above is
    // already committed, so a Keycloak failure here must never fail this
    // response; the Owner can re-trigger by re-issuing the invite
    // (`ensure_user`/`send_set_password_email` are idempotent). Model B never
    // grants membership here: that only happens at first OIDC login, off the
    // group claim (see `admin_invites_accept`'s split below).
    if let Some(kc) = &st.keycloak {
        let group = match req.role {
            crate::tenant::Role::Admin => "quark-admins",
            // `Role::Owner` is rejected above; Member and Viewer both land in
            // the default-closed readers group so every invited role is
            // admitted by the group gate `provision_tenant_keycloak` writes
            // (`required_value: Some("quark-readers")`).
            crate::tenant::Role::Member | crate::tenant::Role::Viewer => "quark-readers",
            crate::tenant::Role::Owner => unreachable!("owner invites are rejected above"),
        };
        match st.store.get_tenant(p.tenant).await {
            Ok(Some(tenant)) => match kc.ensure_user(&tenant.slug, &email, group).await {
                Ok(kc_user_id) => {
                    if let Err(e) = kc.send_set_password_email(&tenant.slug, &kc_user_id).await {
                        log_keycloak_step_error(tenant.id.0, "send_set_password_email", e);
                    }
                }
                Err(e) => log_keycloak_step_error(tenant.id.0, "ensure_user", e),
            },
            Ok(None) => log_keycloak_step_error(p.tenant.0, "get_tenant", "tenant not found"),
            Err(e) => log_keycloak_step_error(p.tenant.0, "get_tenant", e),
        }
    }
    Json(CreateInviteResp {
        id,
        token,
        email,
        role: req.role,
        expires,
    })
    .into_response()
}

#[derive(Serialize)]
struct InviteView {
    id: u64,
    email: String,
    role: crate::tenant::Role,
    expires: u64,
    created: u64,
}

/// `GET /admin/invites`: pending invites for the caller's tenant (cloud only,
/// Owner/Admin required). Never includes `token_hash`.
async fn admin_invites_list(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !st.multi_tenant {
        return StatusCode::NOT_FOUND.into_response();
    }
    let p = match admin_guard(&st, &headers, Scope::Full).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    match st.store.list_invites(p.tenant).await {
        Ok(invites) => Json(
            invites
                .iter()
                .map(|i| InviteView {
                    id: i.id,
                    email: i.email.clone(),
                    role: i.role,
                    expires: i.expires,
                    created: i.created,
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// `DELETE /admin/invites/:id`: revoke a pending invite for the caller's
/// tenant (cloud only, Owner/Admin required). `delete_invite` itself is not
/// row-count aware, so existence is checked against `list_invites` first to
/// give a real `404` for an unknown or already-consumed id.
async fn admin_invites_delete(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<u64>,
) -> Response {
    if !st.multi_tenant {
        return StatusCode::NOT_FOUND.into_response();
    }
    let p = match admin_guard(&st, &headers, Scope::Full).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    let existing = match st.store.list_invites(p.tenant).await {
        Ok(list) => list,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    if !existing.iter().any(|i| i.id == id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    if st.store.delete_invite(p.tenant, id).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    StatusCode::OK.into_response()
}

// --- Per-tenant OIDC config CRUD (multi-tenancy P2d Task 2), cloud-only ---

#[derive(Deserialize)]
struct PutOidcConfigReq {
    issuer: String,
    client_id: String,
    client_secret: String,
    #[serde(default)]
    scopes: Vec<String>,
    #[serde(default)]
    admin_claim: String,
    #[serde(default)]
    admin_value: String,
    #[serde(default)]
    readonly_value: String,
    /// Optional required-group gate (multi-tenancy P2d Task 4b): see
    /// `oidc::passes_required_group`. Not secret, so it is also readable back
    /// on `GET /admin/oidc-config` (`OidcConfigView`).
    #[serde(default)]
    required_value: Option<String>,
    #[serde(default)]
    post_login_url: Option<String>,
}

/// The redacted view of a tenant's OIDC config: every field except
/// `client_secret`, which never leaves the server after being written —
/// `client_secret_set` tells the panel whether one is on file.
#[derive(Serialize)]
struct OidcConfigView {
    issuer: String,
    client_id: String,
    scopes: Vec<String>,
    admin_claim: String,
    admin_value: String,
    readonly_value: String,
    required_value: Option<String>,
    post_login_url: Option<String>,
    client_secret_set: bool,
}

impl From<&crate::oidc::TenantOidcConfig> for OidcConfigView {
    fn from(cfg: &crate::oidc::TenantOidcConfig) -> Self {
        OidcConfigView {
            issuer: cfg.issuer.clone(),
            client_id: cfg.client_id.clone(),
            scopes: cfg.scopes.clone(),
            admin_claim: cfg.admin_claim.clone(),
            admin_value: cfg.admin_value.clone(),
            readonly_value: cfg.readonly_value.clone(),
            required_value: cfg.required_value.clone(),
            post_login_url: cfg.post_login_url.clone(),
            client_secret_set: !cfg.client_secret.is_empty(),
        }
    }
}

/// `PUT /admin/oidc-config`: upserts the caller's tenant's own OIDC IdP
/// (cloud only, Owner/Admin required). Returns the redacted view — never the
/// `client_secret` that was just written.
async fn admin_oidc_config_put(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<PutOidcConfigReq>,
) -> Response {
    if !st.multi_tenant {
        return StatusCode::NOT_FOUND.into_response();
    }
    let p = match admin_guard(&st, &headers, Scope::Full).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    let ip = client_ip(&headers, &st.real_ip_header, None);
    if !st.ratelimiter.check(&ip, now()).await {
        return (StatusCode::TOO_MANY_REQUESTS, "too many requests").into_response();
    }
    let issuer = req.issuer.trim().trim_end_matches('/').to_string();
    let client_id = req.client_id.trim().to_string();
    if issuer.is_empty() || client_id.is_empty() {
        return (StatusCode::BAD_REQUEST, "issuer and client_id are required").into_response();
    }
    let cfg = crate::oidc::TenantOidcConfig {
        tenant_id: p.tenant,
        issuer,
        client_id,
        client_secret: req.client_secret,
        scopes: req.scopes,
        admin_claim: req.admin_claim,
        admin_value: req.admin_value,
        readonly_value: req.readonly_value,
        required_value: req.required_value,
        post_login_url: req.post_login_url,
    };
    if let Err(e) = st.store.put_oidc_config(&cfg).await {
        return conflict_or_503(e).into_response();
    }
    st.oidc_tenants.invalidate(p.tenant).await;
    Json(OidcConfigView::from(&cfg)).into_response()
}

/// `GET /admin/oidc-config`: the caller's tenant's own OIDC IdP, redacted
/// (cloud only, Owner/Admin required). `404` when the tenant has none set up.
async fn admin_oidc_config_get(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !st.multi_tenant {
        return StatusCode::NOT_FOUND.into_response();
    }
    let p = match admin_guard(&st, &headers, Scope::Full).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    match st.store.get_oidc_config(p.tenant).await {
        Ok(Some(cfg)) => Json(OidcConfigView::from(&cfg)).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// `DELETE /admin/oidc-config`: removes the caller's tenant's own OIDC IdP
/// (cloud only, Owner/Admin required). The tenant goes back to having no OIDC
/// of its own; `404` when there was nothing to remove.
async fn admin_oidc_config_delete(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !st.multi_tenant {
        return StatusCode::NOT_FOUND.into_response();
    }
    let p = match admin_guard(&st, &headers, Scope::Full).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    match st.store.get_oidc_config(p.tenant).await {
        Ok(Some(_)) => {}
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    let resp = match st.store.delete_oidc_config(p.tenant).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    st.oidc_tenants.invalidate(p.tenant).await;
    resp
}

#[derive(Serialize)]
struct AcceptInviteResp {
    tenant_id: u64,
    role: crate::tenant::Role,
}

/// `POST /admin/invites/:token/accept` (cloud only): the invited user
/// redeems the token, joining the invite's tenant at the invited role.
///
/// Reached via `session_user_id`, not `admin_guard`: a brand-new user with
/// zero memberships anywhere must still be able to accept an invite (the
/// same reasoning as `admin_tenants_create`). The tenant and role granted
/// ALWAYS come from the invite row, never from the request: the token is
/// the only thing the caller supplies.
///
/// Checks run in an order where no membership is ever granted on a failure
/// path: cloud gate, session, rate limit, invite lookup (covers unknown,
/// expired, and already-accepted tokens alike, since `get_invite_by_hash`
/// hides all three), email match, existing-membership conflict, then the
/// single-winner claim on the invite row itself. `mark_invite_accepted` runs
/// before `put_membership` so the DB row is the atomic single-use gate: two
/// concurrent accepts of the same token both pass the checks above, but only
/// one wins the claim, and only the winner grants membership.
async fn admin_invites_accept(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> Response {
    if !st.multi_tenant {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(user_id) = session_user_id(&st, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let ip = client_ip(&headers, &st.real_ip_header, None);
    if !st.ratelimiter.check(&ip, now()).await {
        return (StatusCode::TOO_MANY_REQUESTS, "too many requests").into_response();
    }
    let inv = match st
        .store
        .get_invite_by_hash(&hash_token(&token), now())
        .await
    {
        Ok(Some(inv)) => inv,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let user = match st.store.get_user_by_id(user_id).await {
        Ok(Some(u)) => u,
        Ok(None) => return StatusCode::UNAUTHORIZED.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    // The invite's email is already lowercased at creation time; the
    // session's user email is normalized the same way here so casing alone
    // never causes a false mismatch.
    if user.email.to_ascii_lowercase() != inv.email {
        return StatusCode::FORBIDDEN.into_response();
    }
    // Model B split (multi-tenancy P2e Task 3): with a `KeycloakAdmin`
    // configured, membership is born at first OIDC login off the group claim
    // (the P2d-A callback), never from accepting an invite. The checks above
    // already confirmed the token is valid and belongs to this session's
    // email, so this is a legitimate invite, but acceptance stops here: no
    // `mark_invite_accepted` claim, no `put_membership`. The invite stays
    // pending (re-acceptable) and the caller is pointed at their org's login
    // instead. Model A (no Keycloak, P2c) is completely unchanged below.
    if st.keycloak.is_some() {
        let slug = match st.store.get_tenant(inv.tenant_id).await {
            Ok(Some(t)) => t.slug,
            // Data-integrity gap, not a backend failure: the invite points at
            // a tenant that no longer exists. Fall back to an empty org hint
            // rather than fail the whole accept.
            Ok(None) => String::new(),
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        };
        return Json(serde_json::json!({
            "status": "login_required",
            "login_url": format!("/admin/login?org={slug}"),
        }))
        .into_response();
    }
    match st.store.get_membership(user_id, inv.tenant_id).await {
        Ok(Some(_)) => return StatusCode::CONFLICT.into_response(),
        Ok(None) => {}
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    match st.store.mark_invite_accepted(inv.id, user_id, now()).await {
        Ok(true) => {}
        // Lost the race, or the invite was already consumed between the
        // lookup above and here: treat it the same as an unknown token.
        Ok(false) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    if st
        .store
        .put_membership(&crate::tenant::Membership {
            user_id,
            tenant_id: inv.tenant_id,
            role: inv.role,
            created: now(),
        })
        .await
        .is_err()
    {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    set_session_tenant(&st, &headers, inv.tenant_id).await;
    Json(AcceptInviteResp {
        tenant_id: inv.tenant_id.0,
        role: inv.role,
    })
    .into_response()
}

/// Name of the short-lived cookie holding the signed Sheets OAuth `state`,
/// binding the connect flow to the browser that started it (anti login-CSRF).
const SHEETS_STATE_COOKIE: &str = "qk_sheets_state";

/// `GET /admin/integrations/sheets/connect`: begin the Google OAuth connect.
/// Called by the panel via `fetch` with its admin credential, so it returns the
/// Google consent URL as JSON (rather than a 303) and sets a signed, short-lived
/// `state` cookie; the panel then navigates the browser to that URL. Returning
/// JSON lets a token-authenticated operator start the flow (a top-level redirect
/// could not carry the `x-admin-token` header). Returns the admin-surface
/// not-found status when the connector is not configured.
async fn sheets_connect(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let p = match admin_guard(&st, &headers, Scope::Full).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    let Some(cfg) = st.sheets.as_ref() else {
        return sheets_off_status(&st).into_response();
    };
    // The random `state` goes to Google in the URL; a signed copy is ALSO stored
    // in a short-lived HttpOnly cookie. The callback requires both to match, so
    // the state is bound to THIS browser and cannot be replayed by an attacker who
    // merely observes a leaked `state` value (login-CSRF). This is the same
    // double-submit binding the OIDC login flow uses.
    //
    // The signed cookie's otherwise-unused `verifier` slot carries the calling
    // principal's tenant (as a decimal string) across the top-level redirect to
    // Google and back, so `sheets_callback` persists the connection under the
    // SAME tenant that started the flow (Host->tenant resolution is P3; until
    // then this is the only way the callback — which carries no admin
    // credential — learns the tenant).
    let state = crate::oidc::random_token();
    let signed =
        crate::oidc::sign_login_state(&st.signing_key, &state, &p.tenant.0.to_string(), "", None);
    let url = crate::sheets::connect_url(cfg, &state);
    let secure = if request_is_https(&headers) {
        "; Secure"
    } else {
        ""
    };
    let cookie = format!(
        "{SHEETS_STATE_COOKIE}={signed}; Max-Age=600; Path=/; HttpOnly; SameSite=Lax{secure}"
    );
    (
        StatusCode::OK,
        [
            (header::SET_COOKIE, cookie),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        Json(serde_json::json!({ "url": url })),
    )
        .into_response()
}

#[derive(Deserialize)]
struct SheetsCallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// Decodes the `email` claim from a Google id_token WITHOUT verifying the
/// signature: the token came straight from Google's token endpoint over TLS, so
/// it is trusted here. Returns `""` on a missing or malformed token.
fn email_from_id_token(id_token: Option<&str>) -> String {
    let Some(token) = id_token else {
        return String::new();
    };
    let mut parts = token.split('.');
    let (_, payload_b64, _) = (parts.next(), parts.next(), parts.next());
    let Some(payload_b64) = payload_b64 else {
        return String::new();
    };
    let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload_b64) else {
        return String::new();
    };
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|v| v.get("email").and_then(|e| e.as_str()).map(str::to_string))
        .unwrap_or_default()
}

/// `GET /admin/integrations/sheets/callback`: verify the state cookie matches
/// the echoed `state`, exchange the code for tokens, persist a connection
/// (requiring a refresh token), clear the state cookie, and redirect to the panel.
async fn sheets_callback(
    State(st): State<Arc<AppState>>,
    Query(params): Query<SheetsCallbackParams>,
    headers: HeaderMap,
) -> Response {
    // No `admin_guard` here: this is a top-level browser redirect from Google and
    // carries no admin credential. It is authorized by a double-submit check on
    // the `state`: the signed cookie set at `/connect` (only an authenticated
    // admin gets one) must decode to the same random value the query echoes back.
    // The cookie binds the flow to THIS browser, so a leaked/observed `state`
    // cannot be replayed by an attacker to inject their own Google account.
    let Some(cfg) = st.sheets.as_ref() else {
        return sheets_off_status(&st).into_response();
    };
    if params.error.is_some() {
        return (StatusCode::UNAUTHORIZED, "connect was denied at Google").into_response();
    }
    // The cookie holds the signed state; the query echoes the raw random value.
    // The `verifier` slot carries the tenant that started the flow (see
    // `sheets_connect`); it comes from the SAME HMAC-verified cookie as the
    // state itself, so it is exactly as trustworthy.
    let verified = cookie_value(&headers, SHEETS_STATE_COOKIE)
        .and_then(|c| crate::oidc::verify_login_state(&st.signing_key, c));
    let cookie_state = verified.as_ref().map(|(state, _, _, _)| state.as_str());
    let tenant = verified
        .as_ref()
        .and_then(|(_, verifier, _, _)| verifier.parse::<u64>().ok())
        .map(crate::tenant::TenantId)
        .unwrap_or(crate::tenant::DEFAULT_TENANT);
    let matches = match (cookie_state, params.state.as_deref()) {
        (Some(a), Some(b)) => constant_time_eq(a.as_bytes(), b.as_bytes()),
        _ => false,
    };
    if !matches {
        return (StatusCode::BAD_REQUEST, "missing or invalid connect state").into_response();
    }
    let Some(code) = params.code else {
        return (StatusCode::BAD_REQUEST, "missing code").into_response();
    };
    let token = match crate::sheets::exchange_code(&reqwest_client(), cfg, &code).await {
        Ok(t) => t,
        Err(_) => return (StatusCode::BAD_GATEWAY, "token exchange failed").into_response(),
    };
    let email = email_from_id_token(token.id_token.as_deref());
    let refresh_token = match token.refresh_token {
        Some(rt) => rt,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                "no refresh_token — reconnect with prompt=consent",
            )
                .into_response()
        }
    };
    let conn = crate::sheets::SheetsConnection {
        refresh_token,
        email,
        spreadsheet_id: None,
        last_sync: None,
        last_status: crate::sheets::SyncStatus::Never,
    };
    if st.store.put_sheets_connection(tenant, &conn).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let clear = format!("{SHEETS_STATE_COOKIE}=; Max-Age=0; Path=/; HttpOnly; SameSite=Lax");
    (
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, "/".to_string()),
            (header::SET_COOKIE, clear),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
    )
        .into_response()
}

/// The Sheets status the panel renders. Deliberately its own struct (never
/// `SheetsConnection`) so the `refresh_token` can NEVER be serialized here.
#[derive(Serialize)]
struct SheetsStatusResponse {
    connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    spreadsheet_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_sync: Option<u64>,
    last_status: crate::sheets::SyncStatus,
}

/// `GET /admin/integrations/sheets/status`: report connection state for the
/// panel. Never includes the refresh token.
async fn sheets_status(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let p = match admin_guard(&st, &headers, Scope::Full).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    if st.sheets.is_none() {
        return sheets_off_status(&st).into_response();
    }
    let conn = match st.store.get_sheets_connection(p.tenant).await {
        Ok(c) => c,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let body = match conn {
        Some(c) => SheetsStatusResponse {
            connected: true,
            email: Some(c.email),
            spreadsheet_url: c
                .spreadsheet_id
                .map(|id| format!("https://docs.google.com/spreadsheets/d/{id}")),
            last_sync: c.last_sync,
            last_status: c.last_status,
        },
        None => SheetsStatusResponse {
            connected: false,
            email: None,
            spreadsheet_url: None,
            last_sync: None,
            last_status: crate::sheets::SyncStatus::Never,
        },
    };
    Json(body).into_response()
}

/// `POST /admin/integrations/sheets/sync`: run one on-demand sync. Refreshes the
/// access token, syncs the catalog, persists the updated connection, and returns
/// the status JSON. A sync error is surfaced in the status (200), not a 500.
async fn sheets_sync(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let p = match admin_guard(&st, &headers, Scope::Full).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    if let Err(status) = csrf_guard(&headers) {
        return status.into_response();
    }
    let (Some(cfg), Some(api)) = (st.sheets.as_ref(), st.sheets_api.as_ref()) else {
        return sheets_off_status(&st).into_response();
    };
    // Take the same sync lease the scheduled task uses, so an on-demand "Sync now"
    // cannot race a scheduled tick into creating two spreadsheets. A short holder
    // id is enough (LMDB always grants; Postgres serializes across replicas). If
    // another sync holds it, tell the caller to retry rather than double-run.
    let holder = format!("sheets_ondemand_{}", crate::oidc::random_token());
    match st.store.try_acquire_sheets_lease(&holder, 120).await {
        Ok(true) => {}
        Ok(false) => return (StatusCode::CONFLICT, "a sync is already running").into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    let mut conn = match st.store.get_sheets_connection(p.tenant).await {
        Ok(Some(c)) => c,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let base_url = format!("https://{}", sheets_base_host(&st, &headers));
    let client = reqwest_client();
    let sync_result =
        match crate::sheets::refresh_access_token(&client, cfg, &conn.refresh_token).await {
            Ok(access_token) => {
                crate::sheets::sync(
                    &st.store,
                    api.as_ref(),
                    st.key,
                    &base_url,
                    &mut conn,
                    &access_token,
                    now(),
                )
                .await
            }
            Err(e) => Err(e),
        };
    if let Err(e) = sync_result {
        conn.last_status = crate::sheets::SyncStatus::Error(e);
    }
    if st
        .store
        .put_sheets_connection(p.tenant, &conn)
        .await
        .is_err()
    {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let body = SheetsStatusResponse {
        connected: true,
        email: Some(conn.email),
        spreadsheet_url: conn
            .spreadsheet_id
            .map(|id| format!("https://docs.google.com/spreadsheets/d/{id}")),
        last_sync: conn.last_sync,
        last_status: conn.last_status,
    };
    Json(body).into_response()
}

/// `DELETE /admin/integrations/sheets`: disconnect (drops the stored connection).
async fn sheets_disconnect(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let p = match admin_guard(&st, &headers, Scope::Full).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    if let Err(status) = csrf_guard(&headers) {
        return status.into_response();
    }
    if st.sheets.is_none() {
        return sheets_off_status(&st).into_response();
    }
    if st.store.delete_sheets_connection(p.tenant).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    StatusCode::NO_CONTENT.into_response()
}

/// The status returned when the Sheets connector is off (not configured): the
/// same not-found status the rest of the admin surface uses (401 when an admin
/// credential exists, else 404).
fn sheets_off_status(st: &AppState) -> StatusCode {
    if st.admin_token.is_some() || st.oidc_configured {
        StatusCode::UNAUTHORIZED
    } else {
        StatusCode::NOT_FOUND
    }
}

/// The public host quark serves on, for building `short_url`s in the synced
/// sheet: the configured `public_host`, else the request `Host` header, else a
/// placeholder. Mirrors how `is_blocked_target` derives the self host.
fn sheets_base_host(st: &AppState, headers: &HeaderMap) -> String {
    st.public_host
        .clone()
        .or_else(|| {
            headers
                .get(header::HOST)
                .and_then(|v| v.to_str().ok())
                .map(|h| h.to_string())
        })
        .unwrap_or_else(|| "localhost".to_string())
}

/// A short-lived reqwest client for the Sheets OAuth token calls (fixed Google
/// hosts, no redirect following).
fn reqwest_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("reqwest client builds")
}

#[derive(Deserialize)]
struct ListParams {
    after: Option<u64>,
    limit: Option<usize>,
    q: Option<String>,
    tag: Option<String>,
    folder: Option<String>,
    /// `broken` restricts the list to links whose last health probe failed.
    health: Option<String>,
}

/// Health of a link's destination as exposed to the panel (never includes
/// anything sensitive; omitted from a row when the link was never probed).
#[derive(Serialize)]
struct HealthInfo {
    healthy: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<u16>,
    checked_at: u64,
}

#[derive(Serialize)]
struct LinkRow {
    id: u64,
    code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    alias: Option<String>,
    url: String,
    expiry: Option<u64>,
    created: u64,
    tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_visits: Option<u32>,
    visits: u64,
    rules: Vec<Rule>,
    variants: Vec<Variant>,
    #[serde(skip_serializing_if = "Option::is_none")]
    app_ios: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    app_android: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    folder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fallback_url: Option<String>,
    /// Whether the link is password-protected. The hash itself is never exposed.
    has_password: bool,
    /// Destination health from the background checker; omitted when unchecked.
    #[serde(skip_serializing_if = "Option::is_none")]
    health: Option<HealthInfo>,
}

async fn admin_links_list(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(p): Query<ListParams>,
) -> Response {
    let prin = match admin_guard(&st, &headers, Scope::LinksRead).await {
        Ok(prin) => prin,
        Err(status) => return status.into_response(),
    };
    let limit = p
        .limit
        .unwrap_or(DEFAULT_PAGE_LIMIT)
        .clamp(1, MAX_PAGE_LIMIT);
    let q = p.q.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let tag = p
        .tag
        .as_deref()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());
    let tag = tag.as_deref();
    let folder = p.folder.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let broken_only = p.health.as_deref() == Some("broken");
    // The `broken` filter is driven by the health table (a small set),
    // cursor-paginated by id, so each page carries real broken rows (search `q`
    // is ignored for this filter; tag/folder still apply). Otherwise the normal
    // link listing/search runs.
    let (links, next_after): (Vec<(u64, Record)>, Option<u64>) = if broken_only {
        let ids = match st.store.list_broken_link_ids(prin.tenant).await {
            Ok(v) => v,
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        };
        let mut picked: Vec<(u64, Record)> = Vec::new();
        let mut last: Option<u64> = None;
        for id in ids.into_iter().filter(|&id| p.after.is_none_or(|a| id > a)) {
            let rec = match st.store.get_link(prin.tenant, id).await {
                Ok(Some(r)) => r,
                Ok(None) => continue,
                Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
            };
            if let Some(t) = tag {
                if !rec.tags.iter().any(|x| x == t) {
                    continue;
                }
            }
            if let Some(f) = folder {
                if !rec
                    .folder
                    .as_deref()
                    .is_some_and(|x| x.eq_ignore_ascii_case(f))
                {
                    continue;
                }
            }
            last = Some(id);
            picked.push((id, rec));
            if picked.len() == limit {
                break;
            }
        }
        let next = if picked.len() == limit { last } else { None };
        (picked, next)
    } else {
        let links = match q {
            Some(term) => match st
                .store
                .search_links(prin.tenant, term, p.after, limit, tag, folder)
                .await
            {
                Ok(l) => l,
                Err(StoreError::Unsupported) => return StatusCode::NOT_IMPLEMENTED.into_response(),
                Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
            },
            None => match st
                .store
                .list_links(prin.tenant, p.after, limit, tag, folder)
                .await
            {
                Ok(l) => l,
                Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
            },
        };
        let next = if links.len() == limit {
            links.last().map(|(id, _)| *id)
        } else {
            None
        };
        (links, next)
    };
    let alias_map: std::collections::HashMap<u64, String> =
        match st.store.list_aliases(prin.tenant).await {
            Ok(pairs) => pairs.into_iter().map(|(a, id)| (id, a)).collect(),
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        };
    // Fetch health for just this page's ids (not the whole table).
    let page_ids: Vec<u64> = links.iter().map(|(id, _)| *id).collect();
    let health_map: std::collections::HashMap<u64, LinkHealth> =
        match st.store.link_health_for(prin.tenant, &page_ids).await {
            Ok(v) => v.into_iter().collect(),
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        };
    let mut rows: Vec<LinkRow> = Vec::with_capacity(links.len());
    for (id, rec) in links {
        let health = health_map.get(&id);
        let visits = match st.store.visits(prin.tenant, id).await {
            Ok(v) => v,
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        };
        rows.push(LinkRow {
            id,
            code: codec::to_base62(permute::encode(id, st.key)),
            alias: alias_map.get(&id).cloned(),
            url: rec.url,
            expiry: rec.expiry,
            created: rec.created,
            tags: rec.tags,
            max_visits: rec.max_visits,
            visits,
            rules: rec.rules,
            variants: rec.variants,
            app_ios: rec.app_ios,
            app_android: rec.app_android,
            folder: rec.folder,
            fallback_url: rec.fallback_url,
            has_password: rec.password_hash.is_some(),
            health: health.map(|h| HealthInfo {
                healthy: h.healthy,
                status: h.status,
                checked_at: h.checked_at,
            }),
        });
    }
    Json(serde_json::json!({ "links": rows, "next_after": next_after })).into_response()
}

/// `GET /admin/tags`: the distinct set of tags across all links, for the
/// panel's filter control.
async fn admin_tags_list(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let p = match admin_guard(&st, &headers, Scope::LinksRead).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    match st.store.list_tags(p.tenant).await {
        Ok(tags) => {
            let rows: Vec<serde_json::Value> = tags
                .into_iter()
                .map(|(name, count)| serde_json::json!({ "name": name, "count": count }))
                .collect();
            Json(serde_json::json!({ "tags": rows })).into_response()
        }
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// `GET /admin/folders`: the distinct folder names with their link counts, for
/// the panel's folder selector and filter control.
async fn admin_folders_list(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let p = match admin_guard(&st, &headers, Scope::LinksRead).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    match st.store.list_folders(p.tenant).await {
        Ok(folders) => {
            let rows: Vec<serde_json::Value> = folders
                .into_iter()
                .map(|(name, count)| serde_json::json!({ "name": name, "count": count }))
                .collect();
            Json(serde_json::json!({ "folders": rows })).into_response()
        }
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// Resolves the code into (id, optional_alias). If the code is numeric, there's no
/// alias to remove; if it's an alias string, returns the alias to delete alongside it.
///
/// Alias lookup is scoped by the caller's tenant default domain (subdomain on
/// cloud, `SHARED_DOMAIN_ID` on OSS/default tenant) — the same namespace
/// `create` stamps the alias into. See `default_domain_id`, `resolve_code`.
async fn resolve_for_admin(
    st: &AppState,
    tenant: crate::tenant::TenantId,
    code: &str,
) -> Result<Option<(u64, Option<String>)>, StoreError> {
    match codec::from_base62(code) {
        Some(c) if c <= permute::MAX_ID => Ok(Some((permute::decode(c, st.key), None))),
        _ => {
            let domain_id = default_domain_id(st, tenant).await;
            match st.store.get_alias(domain_id, code).await? {
                Some(id) => Ok(Some((id, Some(code.to_string())))),
                None => Ok(None),
            }
        }
    }
}

async fn admin_link_delete(
    State(st): State<Arc<AppState>>,
    Path(code): Path<String>,
    headers: HeaderMap,
) -> Response {
    let p = match admin_guard(&st, &headers, Scope::LinksWrite).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    let (id, alias) = match resolve_for_admin(&st, p.tenant, &code).await {
        Ok(Some(v)) => v,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let rec = match st.store.get_link(p.tenant, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let canonical_code = codec::to_base62(permute::encode(id, st.key));
    let ev = WebhookEvent {
        event_type: EventType::LinkDeleted,
        body: webhook_event_payload(
            EventType::LinkDeleted,
            &canonical_code,
            &rec.url,
            alias.as_deref(),
            rec.expiry,
            rec.created,
            None,
        ),
    };
    let rows = st.webhooks.lifecycle_deliveries(p.tenant, &ev).await;
    if st.store.delete_link_tx(p.tenant, id, &rows).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if let Some(a) = &alias {
        let _ = st.store.delete_alias(p.tenant, a).await;
    }
    st.cache.invalidate(id).await;
    st.webhooks.emit_if_in_memory(ev);
    StatusCode::OK.into_response()
}

async fn admin_link_patch(
    State(st): State<Arc<AppState>>,
    Path(code): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let p = match admin_guard(&st, &headers, Scope::LinksWrite).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    let (id, alias) = match resolve_for_admin(&st, p.tenant, &code).await {
        Ok(Some(v)) => v,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let mut rec = match st.store.get_link(p.tenant, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let patch: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid json").into_response(),
    };
    if let Some(u) = patch.get("url") {
        let s = match u.as_str() {
            Some(s) if is_valid_url(s) => s,
            _ => return (StatusCode::BAD_REQUEST, "invalid url").into_response(),
        };
        let Some(host) = extract_host(s) else {
            return (StatusCode::BAD_REQUEST, "url without host").into_response();
        };
        if st.block_private && is_blocked_target(&host, &headers, &st).await {
            return (StatusCode::FORBIDDEN, "destination not allowed").into_response();
        }
        rec.url = s.to_string();
    }
    if let Some(ttl) = patch.get("ttl") {
        if ttl.is_null() {
            rec.expiry = None;
        } else if let Some(secs) = ttl.as_u64() {
            match now().checked_add(secs) {
                Some(e) => rec.expiry = Some(e),
                None => return (StatusCode::BAD_REQUEST, "invalid ttl").into_response(),
            }
        } else {
            return (StatusCode::BAD_REQUEST, "invalid ttl").into_response();
        }
    }
    if let Some(t) = patch.get("tags") {
        match t {
            serde_json::Value::Array(items) => {
                let mut raw = Vec::with_capacity(items.len());
                for item in items {
                    match item.as_str() {
                        Some(s) => raw.push(s.to_string()),
                        None => return (StatusCode::BAD_REQUEST, "invalid tags").into_response(),
                    }
                }
                rec.tags = normalize_tags(raw);
            }
            _ => return (StatusCode::BAD_REQUEST, "invalid tags").into_response(),
        }
    }
    if let Some(mv) = patch.get("max_visits") {
        if mv.is_null() {
            rec.max_visits = None;
        } else if let Some(n) = mv.as_u64().and_then(|n| u32::try_from(n).ok()) {
            rec.max_visits = normalize_max_visits(Some(n));
        } else {
            return (StatusCode::BAD_REQUEST, "invalid max_visits").into_response();
        }
    }
    if let Some(r) = patch.get("rules") {
        let parsed: Vec<Rule> = match serde_json::from_value(r.clone()) {
            Ok(v) => v,
            Err(_) => return (StatusCode::BAD_REQUEST, "invalid rules").into_response(),
        };
        match validate_rules(parsed, &headers, &st).await {
            Ok(v) => rec.rules = v,
            Err(resp) => return resp,
        }
    }
    if let Some(v) = patch.get("variants") {
        let variants: Vec<Variant> = match serde_json::from_value(v.clone()) {
            Ok(vs) => vs,
            Err(_) => return (StatusCode::BAD_REQUEST, "invalid variants").into_response(),
        };
        if let Err(resp) = validate_variants(&variants, &headers, &st).await {
            return resp;
        }
        rec.variants = variants;
    }
    if let Some(v) = patch.get("app_ios") {
        if v.is_null() {
            rec.app_ios = None;
        } else if let Some(s) = v.as_str() {
            if let Err(status) = app_destination_ok(&st, &headers, s).await {
                return (status, "invalid app destination").into_response();
            }
            rec.app_ios = Some(s.to_string());
        } else {
            return (StatusCode::BAD_REQUEST, "invalid app destination").into_response();
        }
    }
    if let Some(v) = patch.get("app_android") {
        if v.is_null() {
            rec.app_android = None;
        } else if let Some(s) = v.as_str() {
            if let Err(status) = app_destination_ok(&st, &headers, s).await {
                return (status, "invalid app destination").into_response();
            }
            rec.app_android = Some(s.to_string());
        } else {
            return (StatusCode::BAD_REQUEST, "invalid app destination").into_response();
        }
    }
    if let Some(v) = patch.get("folder") {
        if v.is_null() {
            rec.folder = None;
        } else if let Some(s) = v.as_str() {
            rec.folder = normalize_folder(Some(s.to_string()));
        } else {
            return (StatusCode::BAD_REQUEST, "invalid folder").into_response();
        }
    }
    if let Some(v) = patch.get("fallback_url") {
        if v.is_null() {
            rec.fallback_url = None;
        } else if let Some(s) = v.as_str() {
            let s = s.trim();
            if s.is_empty() {
                rec.fallback_url = None;
            } else if let Err(status) = app_destination_ok(&st, &headers, s).await {
                return (status, "invalid fallback url").into_response();
            } else {
                rec.fallback_url = Some(s.to_string());
            }
        } else {
            return (StatusCode::BAD_REQUEST, "invalid fallback url").into_response();
        }
    }
    if let Some(v) = patch.get("password") {
        if v.is_null() {
            rec.password_hash = None;
        } else if let Some(s) = v.as_str() {
            let s = s.trim();
            if s.is_empty() {
                rec.password_hash = None;
            } else {
                let pw = s.to_string();
                match tokio::task::spawn_blocking(move || crate::password::hash_password(&pw)).await
                {
                    Ok(Ok(h)) => rec.password_hash = Some(h),
                    _ => {
                        return (StatusCode::INTERNAL_SERVER_ERROR, "could not hash password")
                            .into_response()
                    }
                }
            }
        } else {
            return (StatusCode::BAD_REQUEST, "invalid password").into_response();
        }
    }
    let canonical_code = codec::to_base62(permute::encode(id, st.key));
    let ev = WebhookEvent {
        event_type: EventType::LinkUpdated,
        body: webhook_event_payload(
            EventType::LinkUpdated,
            &canonical_code,
            &rec.url,
            alias.as_deref(),
            rec.expiry,
            rec.created,
            None,
        ),
    };
    let rows = st.webhooks.lifecycle_deliveries(p.tenant, &ev).await;
    if st
        .store
        .put_link_tx(p.tenant, id, &rec, &rows)
        .await
        .is_err()
    {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    st.cache.invalidate(id).await;
    st.webhooks.emit_if_in_memory(ev);
    StatusCode::OK.into_response()
}

#[derive(Deserialize)]
struct WebhookCreateReq {
    url: String,
    events: Vec<EventType>,
    active: Option<bool>,
    #[serde(default)]
    kind: SubscriptionKind,
}

#[derive(Deserialize)]
struct WebhookPatchReq {
    url: Option<String>,
    events: Option<Vec<EventType>>,
    active: Option<bool>,
    kind: Option<SubscriptionKind>,
}

#[derive(Serialize)]
struct WebhookRow {
    id: u64,
    url: String,
    events: Vec<EventType>,
    active: bool,
    created: u64,
    secret_masked: String,
    kind: SubscriptionKind,
}

async fn admin_webhooks_list(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let p = match admin_guard(&st, &headers, Scope::Webhooks).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    match st.store.list_webhooks(p.tenant).await {
        Ok(subs) => {
            let rows: Vec<WebhookRow> = subs
                .into_iter()
                .map(|s| WebhookRow {
                    id: s.id,
                    url: s.url,
                    events: s.events,
                    active: s.active,
                    created: s.created,
                    secret_masked: mask_secret(&s.secret),
                    kind: s.kind,
                })
                .collect();
            Json(serde_json::json!({ "webhooks": rows })).into_response()
        }
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// Serves a stored well-known document as `application/json`. Public, no auth.
/// Tenant is picked by the incoming `Host` header via `resolve_host_route`,
/// the same resolution the redirect hot path uses: the shared host (or OSS,
/// where `multi_tenant` is off) always lands on `DEFAULT_TENANT`; a verified
/// custom domain serves its own tenant's document; an unknown/unverified
/// host has no route, so this 404s before ever touching the store.
/// `Some(body)` -> 200 verbatim; `None` -> 404; store error -> 503.
async fn serve_wellknown(st: &AppState, name: &str, headers: &HeaderMap) -> Response {
    let Some(route) = resolve_host_route(st, headers).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match st.store.get_wellknown(route.tenant_id, name).await {
        Ok(Some(body)) => ([(header::CONTENT_TYPE, "application/json")], body).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// Maximum number of pixel configs allowed on this instance (instance-level
/// config for this pass, roadmap #14 — not per-link, so this stays small).
const PIXELS_CAP: usize = 20;
/// Placeholder shown instead of a secret credential in `GET /admin/pixels`.
/// The raw value is never sent back once stored.
const MASKED_SECRET: &str = "\u{2022}\u{2022}\u{2022}\u{2022}";

#[derive(Deserialize)]
struct PixelCreateReq {
    provider: Provider,
    credentials: PixelCredentials,
    active: Option<bool>,
}

#[derive(Serialize)]
struct MaskedCredentials {
    measurement_id: Option<String>,
    api_secret: Option<String>,
    pixel_id: Option<String>,
    access_token: Option<String>,
}

/// Masks the secret fields (`api_secret`/`access_token`); `measurement_id`
/// and `pixel_id` are provider-facing identifiers, not secrets, so they pass
/// through unmasked.
fn mask_credentials(c: &PixelCredentials) -> MaskedCredentials {
    MaskedCredentials {
        measurement_id: c.measurement_id.clone(),
        api_secret: c.api_secret.as_ref().map(|_| MASKED_SECRET.to_string()),
        pixel_id: c.pixel_id.clone(),
        access_token: c.access_token.as_ref().map(|_| MASKED_SECRET.to_string()),
    }
}

#[derive(Serialize)]
struct PixelRow {
    id: u64,
    provider: Provider,
    credentials: MaskedCredentials,
    active: bool,
    created: u64,
}

fn to_pixel_row(config: &PixelConfig) -> PixelRow {
    PixelRow {
        id: config.id,
        provider: config.provider,
        credentials: mask_credentials(&config.credentials),
        active: config.active,
        created: config.created,
    }
}

/// Minimal credential validation per provider: GA4 needs measurement_id +
/// api_secret; Meta needs pixel_id + access_token. Both non-empty (trimmed).
fn has_required_pixel_credentials(provider: Provider, c: &PixelCredentials) -> bool {
    fn non_empty(s: &Option<String>) -> bool {
        s.as_deref().map(|v| !v.trim().is_empty()).unwrap_or(false)
    }
    match provider {
        Provider::Ga4 => non_empty(&c.measurement_id) && non_empty(&c.api_secret),
        Provider::MetaCapi => non_empty(&c.pixel_id) && non_empty(&c.access_token),
    }
}

async fn admin_pixels_list(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let p = match admin_guard(&st, &headers, Scope::Analytics).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    match st.store.list_pixels(p.tenant).await {
        Ok(pixels) => {
            let rows: Vec<PixelRow> = pixels.iter().map(to_pixel_row).collect();
            Json(serde_json::json!({ "pixels": rows })).into_response()
        }
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// Maximum number of API tokens that may exist at once.
const MAX_API_TOKENS: usize = 100;

/// Token row shape for `GET /admin/tokens`: never includes the hash or the
/// plaintext, only what an operator needs to recognize/manage a token.
#[derive(Serialize)]
struct ApiTokenRow {
    id: u64,
    name: String,
    scopes: Vec<Scope>,
    rate_limit_per_min: Option<u32>,
    created: u64,
}

impl From<ApiToken> for ApiTokenRow {
    fn from(t: ApiToken) -> Self {
        ApiTokenRow {
            id: t.id,
            name: t.name,
            scopes: t.scopes,
            rate_limit_per_min: t.rate_limit_per_min,
            created: t.created,
        }
    }
}

async fn admin_tokens_list(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let p = match admin_guard(&st, &headers, Scope::Full).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    match st.store.list_api_tokens(p.tenant).await {
        Ok(tokens) => {
            let rows: Vec<ApiTokenRow> = tokens.into_iter().map(ApiTokenRow::from).collect();
            Json(serde_json::json!({ "tokens": rows })).into_response()
        }
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

async fn admin_webhooks_create(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<WebhookCreateReq>,
) -> Response {
    let p = match admin_guard(&st, &headers, Scope::Webhooks).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    if let Err((status, msg)) = validate_webhook_url(&req.url) {
        return (status, msg).into_response();
    }
    let count = match st.store.list_webhooks(p.tenant).await {
        Ok(subs) => subs.len(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    if count >= MAX_WEBHOOK_SUBSCRIPTIONS {
        return (StatusCode::BAD_REQUEST, "webhook subscription cap reached").into_response();
    }
    let id = match st.store.next_webhook_id(p.tenant).await {
        Ok(id) => id,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    // Only Generic subscriptions sign deliveries, so only they need an HMAC
    // secret. Channel kinds (Slack/Discord/Telegram) authenticate via the
    // incoming URL itself and get no secret at all.
    let secret = match req.kind {
        SubscriptionKind::Generic => webhooks::generate_secret(),
        _ => String::new(),
    };
    let sub = WebhookSubscription {
        id,
        url: req.url,
        events: req.events,
        secret: secret.clone(),
        active: req.active.unwrap_or(true),
        created: now(),
        kind: req.kind,
    };
    if st.store.put_webhook(p.tenant, &sub).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let mut resp = serde_json::json!({ "id": id });
    if sub.kind == SubscriptionKind::Generic {
        resp["secret"] = serde_json::Value::String(secret);
    }
    (StatusCode::CREATED, Json(resp)).into_response()
}

async fn admin_webhooks_patch(
    State(st): State<Arc<AppState>>,
    Path(id): Path<u64>,
    headers: HeaderMap,
    Json(req): Json<WebhookPatchReq>,
) -> Response {
    let p = match admin_guard(&st, &headers, Scope::Webhooks).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    let mut sub = match st.store.get_webhook(p.tenant, id).await {
        Ok(Some(s)) => s,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    if let Some(url) = req.url {
        if let Err((status, msg)) = validate_webhook_url(&url) {
            return (status, msg).into_response();
        }
        sub.url = url;
    }
    if let Some(events) = req.events {
        sub.events = events;
    }
    if let Some(active) = req.active {
        sub.active = active;
    }
    if let Some(kind) = req.kind {
        sub.kind = kind;
    }
    // A kind change can strand the secret in a state where it no longer
    // matches the resulting kind: switching a channel (secret="") to
    // Generic would sign with an empty key (silently defeated signing,
    // since `sign("", ...)` does not error); switching a Generic sub to a
    // channel leaves a signing secret with nothing to verify it. Reconcile
    // the secret to the resulting kind, mirroring `admin_webhooks_create`.
    match sub.kind {
        SubscriptionKind::Generic if sub.secret.is_empty() => {
            sub.secret = webhooks::generate_secret();
        }
        SubscriptionKind::Generic => {}
        _ => sub.secret = String::new(),
    }
    if st.store.put_webhook(p.tenant, &sub).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    StatusCode::OK.into_response()
}

async fn wellknown_aasa(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    serve_wellknown(&st, "apple-app-site-association", &headers).await
}

async fn wellknown_assetlinks(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    serve_wellknown(&st, "assetlinks.json", &headers).await
}

async fn admin_wellknown_get(
    State(st): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    let p = match admin_guard(&st, &headers, Scope::Full).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    if !WELLKNOWN_NAMES.contains(&name.as_str()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    match st.store.get_wellknown(p.tenant, &name).await {
        Ok(Some(body)) => ([(header::CONTENT_TYPE, "application/json")], body).into_response(),
        // Admin read of an unset document: 200 with an empty body (the panel
        // treats empty as "not configured"). Avoids a spurious 404 in the
        // browser console on every App Links page load. The public serve path
        // still returns 404 when unset, which is what iOS/Android expect.
        Ok(None) => (StatusCode::OK, "").into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

async fn admin_wellknown_put(
    State(st): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let p = match admin_guard(&st, &headers, Scope::Full).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    if !WELLKNOWN_NAMES.contains(&name.as_str()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    if body.len() > WELLKNOWN_MAX {
        return (StatusCode::BAD_REQUEST, "body too large").into_response();
    }
    let text = match std::str::from_utf8(&body) {
        Ok(t) => t,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid json").into_response(),
    };
    if serde_json::from_str::<serde_json::Value>(text).is_err() {
        return (StatusCode::BAD_REQUEST, "invalid json").into_response();
    }
    if st.store.put_wellknown(p.tenant, &name, text).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    StatusCode::OK.into_response()
}

async fn admin_webhooks_delete(
    State(st): State<Arc<AppState>>,
    Path(id): Path<u64>,
    headers: HeaderMap,
) -> Response {
    let p = match admin_guard(&st, &headers, Scope::Webhooks).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    match st.store.delete_webhook(p.tenant, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

#[derive(Deserialize)]
struct CreateTokenReq {
    name: String,
    scopes: Vec<Scope>,
    rate_limit_per_min: Option<u32>,
}

#[derive(Serialize)]
struct CreateTokenResp {
    id: u64,
    token: String,
}

async fn admin_tokens_create(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let p = match admin_guard(&st, &headers, Scope::Full).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    let req: CreateTokenReq = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid json").into_response(),
    };
    let existing = match st.store.list_api_tokens(p.tenant).await {
        Ok(t) => t,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    if existing.len() >= MAX_API_TOKENS {
        return (StatusCode::BAD_REQUEST, "token cap reached").into_response();
    }
    let id = match st.store.next_api_token_id(p.tenant).await {
        Ok(id) => id,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let plaintext = generate_token();
    let token = ApiToken {
        id,
        name: req.name,
        token_hash: hash_token(&plaintext),
        scopes: req.scopes,
        rate_limit_per_min: req.rate_limit_per_min,
        created: now(),
        tenant_id: p.tenant,
    };
    if st.store.put_api_token(p.tenant, &token).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    (
        StatusCode::CREATED,
        Json(CreateTokenResp {
            id,
            token: plaintext,
        }),
    )
        .into_response()
}

async fn admin_tokens_delete(
    State(st): State<Arc<AppState>>,
    Path(id): Path<u64>,
    headers: HeaderMap,
) -> Response {
    let p = match admin_guard(&st, &headers, Scope::Full).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    match st.store.delete_api_token(p.tenant, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

async fn admin_pixels_create(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let p = match admin_guard(&st, &headers, Scope::Analytics).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    let req: PixelCreateReq = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid json").into_response(),
    };
    if !has_required_pixel_credentials(req.provider, &req.credentials) {
        return (
            StatusCode::BAD_REQUEST,
            "missing required credentials for provider",
        )
            .into_response();
    }
    let existing = match st.store.list_pixels(p.tenant).await {
        Ok(pixels) => pixels,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    if existing.len() >= PIXELS_CAP {
        return (StatusCode::BAD_REQUEST, "pixel config limit reached (20)").into_response();
    }
    let id = match st.store.next_pixel_id(p.tenant).await {
        Ok(id) => id,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let config = PixelConfig {
        id,
        provider: req.provider,
        credentials: req.credentials,
        active: req.active.unwrap_or(true),
        created: now(),
    };
    if st.store.put_pixel(p.tenant, &config).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    (StatusCode::CREATED, Json(to_pixel_row(&config))).into_response()
}

async fn admin_pixels_delete(
    State(st): State<Arc<AppState>>,
    Path(id): Path<u64>,
    headers: HeaderMap,
) -> Response {
    let p = match admin_guard(&st, &headers, Scope::Analytics).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    match st.store.delete_pixel(p.tenant, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// Sends a synthetic `link.created` event straight to a single subscription,
/// bypassing the queue: unlike `emit`, the caller (an admin, testing their
/// endpoint) wants to see the outcome, so this delivers once, synchronously,
/// and reports the result instead of fire-and-forget.
async fn admin_webhooks_test(
    State(st): State<Arc<AppState>>,
    Path(id): Path<u64>,
    headers: HeaderMap,
) -> Response {
    let p = match admin_guard(&st, &headers, Scope::Webhooks).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    // Bare POST (no body) is a cross-site "simple" request; require the custom
    // header so the SameSite=None session cookie can't be used to fire tests.
    if let Err(status) = csrf_guard(&headers) {
        return status.into_response();
    }
    let sub = match st.store.get_webhook(p.tenant, id).await {
        Ok(Some(s)) => s,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    send_test_event_guarded(&sub, is_internal_host).await
}

/// Core of `admin_webhooks_test`, with the SSRF host-block predicate
/// injected. Production always calls this through `admin_webhooks_test`,
/// which wires in the real `is_internal_host`; unit tests exercise real HTTP
/// delivery (kind-branching, signing, headers) against a local test server
/// via this seam with a permissive predicate, since every loopback/private
/// address a local test server can bind to is, correctly, always blocked by
/// `is_internal_host` (mirrors `webhooks::delivery::deliver_to_matching_guarded`).
async fn send_test_event_guarded(
    sub: &WebhookSubscription,
    is_blocked: impl Fn(&str) -> bool,
) -> Response {
    // SSRF guard applies to the test-send too: an admin-controlled URL is
    // still an operator-supplied URL, and this endpoint fires synchronously
    // instead of through the queue's own guard (see
    // `webhooks::delivery::deliver_to_matching_guarded`).
    let host = match extract_host(&sub.url) {
        Some(h) => h,
        None => return (StatusCode::BAD_REQUEST, "invalid webhook url").into_response(),
    };
    if is_blocked(&host) {
        return (
            StatusCode::BAD_REQUEST,
            "webhook url resolves to an internal host",
        )
            .into_response();
    }
    let body = webhook_event_payload(
        EventType::LinkCreated,
        "TEST0000",
        "https://example.com/test",
        None,
        None,
        now(),
        None,
    );
    let ev = WebhookEvent {
        event_type: EventType::LinkCreated,
        body,
    };
    // Same request-shaping as a real delivery (`deliver_one`): Generic gets
    // a signed envelope, channel kinds get an unsigned, channel-formatted
    // payload. This is what review Task 1 of #6 required — the test-send
    // must exercise the same branch a real event would take.
    let req = match webhooks::delivery::build_outgoing_request(sub, &ev, None) {
        Some(r) => r,
        None => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(WEBHOOK_TEST_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(c) => c,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let mut builder = client
        .post(&sub.url)
        .header("content-type", "application/json");
    for (name, value) in &req.extra_headers {
        builder = builder.header(*name, value);
    }
    let result = builder.body(req.body).send().await;
    match result {
        Ok(resp) => Json(serde_json::json!({
            "delivered": resp.status().is_success(),
            "status": resp.status().as_u16(),
        }))
        .into_response(),
        Err(e) => Json(serde_json::json!({
            "delivered": false,
            "error": e.to_string(),
        }))
        .into_response(),
    }
}

async fn admin_wellknown_delete(
    State(st): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    let p = match admin_guard(&st, &headers, Scope::Full).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    if !WELLKNOWN_NAMES.contains(&name.as_str()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    if st.store.delete_wellknown(p.tenant, &name).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    StatusCode::NO_CONTENT.into_response()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

async fn health() -> &'static str {
    "ok"
}

/// Formats an access log line as JSON. Pure function: no I/O, easy to test.
fn access_log_line(method: &str, path: &str, status: u16, latency_ms: f64) -> String {
    let latency_ms = (latency_ms * 1000.0).round() / 1000.0;
    serde_json::json!({
        "method": method,
        "path": path,
        "status": status,
        "latency_ms": latency_ms,
    })
    .to_string()
}

/// Middleware that logs one JSON line per request to stdout (Coolify captures stdout).
/// Purely observational: doesn't alter the response.
async fn log_requests(req: Request, next: Next) -> Response {
    let start = Instant::now();
    let method = req.method().to_string();
    let path = req.uri().path().to_string();

    let response = next.run(req).await;

    let latency_ms = start.elapsed().as_secs_f64() * 1000.0;
    let status = response.status().as_u16();
    println!("{}", access_log_line(&method, &path, status, latency_ms));

    response
}

/// CORS origins from the `QUARK_CORS_ORIGINS` env var (comma-separated list).
pub fn parse_cors_origins(raw: Option<String>) -> Vec<String> {
    match raw {
        None => Vec::new(),
        Some(s) => s
            .split(',')
            .map(|o| o.trim().to_string())
            .filter(|o| !o.is_empty())
            .collect(),
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    let origins = parse_cors_origins(std::env::var("QUARK_CORS_ORIGINS").ok());
    router_with_cors(state, origins)
}

pub fn router_with_cors(state: Arc<AppState>, origins: Vec<String>) -> Router {
    let app = Router::new()
        .route("/", post(create))
        .route("/health", get(health))
        .route("/:code", get(redirect).post(unlock))
        .route("/:code/stats", get(stats))
        .route("/admin/stats", get(admin_stats))
        .route("/admin/links", get(admin_links_list))
        .route("/admin/import", post(admin_import))
        .route(
            "/admin/links/:code",
            axum::routing::delete(admin_link_delete).patch(admin_link_patch),
        )
        .route(
            "/admin/webhooks",
            get(admin_webhooks_list).post(admin_webhooks_create),
        )
        .route(
            "/admin/webhooks/:id",
            axum::routing::patch(admin_webhooks_patch).delete(admin_webhooks_delete),
        )
        .route("/admin/webhooks/:id/test", post(admin_webhooks_test))
        .route("/admin/login", get(oidc_login))
        .route("/admin/callback", get(oidc_callback))
        .route("/admin/logout", post(oidc_logout))
        .route("/admin/me", get(admin_me))
        .route("/admin/tenants", post(admin_tenants_create))
        .route("/admin/workspace/switch", post(admin_workspace_switch))
        .route(
            "/admin/oidc-config",
            get(admin_oidc_config_get)
                .put(admin_oidc_config_put)
                .delete(admin_oidc_config_delete),
        )
        .route(
            "/admin/domains",
            get(admin_domains_list).post(admin_domains_create),
        )
        .route(
            "/admin/domains/:id",
            axum::routing::delete(admin_domains_delete),
        )
        .route("/admin/domains/:id/verify", post(admin_domains_verify))
        .route(
            "/admin/sso-domains",
            get(admin_sso_domains_list).post(admin_sso_domains_create),
        )
        .route(
            "/admin/sso-domains/:id",
            axum::routing::delete(admin_sso_domains_delete),
        )
        .route(
            "/admin/sso-domains/:id/verify",
            post(admin_sso_domains_verify),
        )
        .route("/admin/sso/discover", get(sso_discover))
        .route(
            "/admin/invites",
            get(admin_invites_list).post(admin_invites_create),
        )
        .route(
            "/admin/invites/:id",
            axum::routing::delete(admin_invites_delete),
        )
        .route("/admin/invites/:token/accept", post(admin_invites_accept))
        .route("/admin/integrations/sheets/connect", get(sheets_connect))
        .route("/admin/integrations/sheets/callback", get(sheets_callback))
        .route("/admin/integrations/sheets/sync", post(sheets_sync))
        .route("/admin/integrations/sheets/status", get(sheets_status))
        .route(
            "/admin/integrations/sheets",
            axum::routing::delete(sheets_disconnect),
        )
        .route("/admin/tags", get(admin_tags_list))
        .route("/admin/folders", get(admin_folders_list))
        .route(
            "/admin/tokens",
            get(admin_tokens_list).post(admin_tokens_create),
        )
        .route(
            "/admin/tokens/:id",
            axum::routing::delete(admin_tokens_delete),
        )
        .route(
            "/admin/pixels",
            get(admin_pixels_list).post(admin_pixels_create),
        )
        .route(
            "/admin/pixels/:id",
            axum::routing::delete(admin_pixels_delete),
        )
        .route(
            "/.well-known/apple-app-site-association",
            get(wellknown_aasa),
        )
        .route("/apple-app-site-association", get(wellknown_aasa))
        .route("/.well-known/assetlinks.json", get(wellknown_assetlinks))
        .route(
            "/admin/wellknown/:name",
            get(admin_wellknown_get)
                .put(admin_wellknown_put)
                .delete(admin_wellknown_delete),
        )
        .with_state(state);

    let app = if origins.is_empty() {
        app
    } else {
        let list: Vec<axum::http::HeaderValue> =
            origins.iter().filter_map(|o| o.parse().ok()).collect();
        let cors = CorsLayer::new()
            .allow_origin(list)
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::PATCH,
                Method::DELETE,
            ])
            // Specific headers (not `Any`) because credentials are allowed, and
            // `*` is invalid with credentials. Allow credentials so the OIDC
            // session cookie is accepted on a cross-origin panel.
            .allow_headers([
                header::CONTENT_TYPE,
                axum::http::HeaderName::from_static("x-admin-token"),
                axum::http::HeaderName::from_static("x-quark-csrf"),
            ])
            .allow_credentials(true);
        app.layer(cors)
    };

    if std::env::var("QUARK_ACCESS_LOG").is_ok() {
        app.layer(axum::middleware::from_fn(log_requests))
    } else {
        app
    }
}

#[cfg(test)]
mod tests {
    use super::{
        access_log_line, app_destination, cache_control_for, classify_platform, create_link_core,
        fbclid_from_query, normalize_max_visits, parse_cors_origins, resolve_code,
        resolve_for_admin, send_test_event_guarded, EventType, Platform, SubscriptionKind,
        WebhookSubscription, SHARED_DOMAIN_ID,
    };
    use crate::store::Record;
    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::HeaderMap as ReqHeaderMap;
    use axum::routing::any;
    use axum::Router as TestRouter;
    use std::sync::Arc;
    use std::sync::Mutex;
    use tokio::net::TcpListener;

    fn rec(app_ios: Option<&str>, app_android: Option<&str>) -> Record {
        Record {
            url: "https://example.com".into(),
            expiry: None,
            created: 0,
            tags: Vec::new(),
            max_visits: None,
            rules: Vec::new(),
            variants: Vec::new(),
            app_ios: app_ios.map(str::to_string),
            app_android: app_android.map(str::to_string),
            folder: None,
            fallback_url: None,
            password_hash: None,
            tenant_id: crate::tenant::DEFAULT_TENANT,
        }
    }

    /// Minimal `AppState` for exercising `admin_guard` directly: LMDB-backed
    /// store (so API tokens can be inserted), no OIDC/sheets, rate limiter
    /// disabled. `admin_token` sets (or clears) the env break-glass token.
    async fn guard_state(admin_token: Option<&str>) -> Arc<super::AppState> {
        guard_state_with_oidc(admin_token, false).await
    }

    /// Same as `guard_state`, but lets the caller control `oidc_configured`
    /// (needed to exercise the OIDC-gated session paths, e.g.
    /// `session_user_id`, without wiring a real IdP).
    async fn guard_state_with_oidc(
        admin_token: Option<&str>,
        oidc_configured: bool,
    ) -> Arc<super::AppState> {
        let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
        let (store, sink) = crate::store::open_backends(dir.path(), false)
            .await
            .unwrap();
        let cache = crate::cache::Cache::new(store.clone(), 1000, None);
        let host_router = Arc::new(crate::domain_router::HostRouter::new(
            store.clone(),
            None,
            None,
        ));
        let (analytics_tx, _rx) = tokio::sync::mpsc::channel(100);
        let (tx, _wrx) = tokio::sync::mpsc::channel(1);
        let webhooks = Arc::new(crate::webhooks::delivery::WebhookDispatcher::new(
            tx,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        ));
        Arc::new(super::AppState {
            oidc: None,
            sheets: None,
            sheets_api: None,
            oidc_configured,
            cache,
            store,
            key: 0x1234,
            signing_key: [0u8; 32],
            analytics_tx,
            sink,
            admin_token: admin_token.map(str::to_string),
            ratelimiter: crate::abuse::ratelimit::RateLimiter::disabled(),
            block_private: true,
            public_host: None,
            real_ip_header: "cf-connecting-ip".to_string(),
            webhooks,
            multi_tenant: false,
            host_router,
            dns: Arc::new(crate::dns::NullDns),
            tenant_domain_suffix: None,
            oidc_tenants: crate::oidc::TenantOidcCache::new(),
            keycloak: None,
            keycloak_base_url: None,
        })
    }

    /// Cloud-mode `AppState` for exercising the `?org=` login/callback
    /// decision logic (multi-tenancy P2d) without a live IdP: LMDB-backed
    /// (`multi_tenant: true`), no global env OIDC configured. LMDB's
    /// `get_oidc_config_bare`/`get_oidc_config` always return `Ok(None)` (see
    /// `src/store/lmdb.rs`), which is exactly the "tenant exists but has no
    /// IdP of its own" shape these tests need — the store-lookup and
    /// tenant-resolution branches never require reaching a real IdP over the
    /// network.
    async fn multi_tenant_state() -> Arc<super::AppState> {
        let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
        let (store, sink) = crate::store::open_backends(dir.path(), false)
            .await
            .unwrap();
        let cache = crate::cache::Cache::new(store.clone(), 1000, None);
        let host_router = Arc::new(crate::domain_router::HostRouter::new(
            store.clone(),
            None,
            None,
        ));
        let (analytics_tx, _rx) = tokio::sync::mpsc::channel(100);
        let (tx, _wrx) = tokio::sync::mpsc::channel(1);
        let webhooks = Arc::new(crate::webhooks::delivery::WebhookDispatcher::new(
            tx,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        ));
        Arc::new(super::AppState {
            oidc: None,
            sheets: None,
            sheets_api: None,
            oidc_configured: false,
            cache,
            store,
            key: 0x1234,
            signing_key: [7u8; 32],
            analytics_tx,
            sink,
            admin_token: None,
            ratelimiter: crate::abuse::ratelimit::RateLimiter::disabled(),
            block_private: true,
            public_host: None,
            real_ip_header: "cf-connecting-ip".to_string(),
            webhooks,
            multi_tenant: true,
            host_router,
            dns: Arc::new(crate::dns::NullDns),
            tenant_domain_suffix: None,
            oidc_tenants: crate::oidc::TenantOidcCache::new(),
            keycloak: None,
            keycloak_base_url: None,
        })
    }

    // --- `?org=` login / per-tenant callback (multi-tenancy P2d) ---
    //
    // These exercise the resolution/decision logic directly against the
    // handlers: which tenant (if any) a login resolves to, and whether the
    // outcome is the explicit error the security model requires (never a
    // silent fallthrough to a different IdP). None of the cases below need a
    // live IdP: `?org=` on an unknown slug or a tenant with no config of its
    // own is rejected before any network call would happen. The "known slug
    // WITH a working config" happy path additionally needs the tenant's IdP
    // to actually answer discovery/JWKS/token requests, which the LMDB test
    // backend has no way to provide (`get_oidc_config_bare` always returns
    // `Ok(None)` there); that path is covered by the Postgres-gated store
    // tests (`tests/oidc_config_it.rs`) for config storage/isolation, plus
    // the `oidc.rs` unit tests for cookie signing/claim mapping/membership.
    // Exercising the full network round trip needs a real or fake IdP
    // (Keycloak, per `docker-compose.e2e.yml`) and is deferred to the P2d
    // frontend/e2e follow-up; see the Task 4 report.

    #[tokio::test]
    async fn org_login_unknown_slug_is_404_not_global() {
        let st = multi_tenant_state().await;
        let resp = super::oidc_login(
            State(st),
            axum::extract::Query(super::LoginParams {
                org: Some("ghost-org".to_string()),
            }),
            ReqHeaderMap::new(),
        )
        .await;
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
        assert!(
            resp.headers().get(axum::http::header::LOCATION).is_none(),
            "an unknown org must never redirect to any IdP"
        );
    }

    #[tokio::test]
    async fn org_login_tenant_without_oidc_config_is_404_not_global() {
        let st = multi_tenant_state().await;
        let tenant_id = crate::tenant::TenantId(st.store.next_tenant_id().await.unwrap());
        st.store
            .put_tenant(&crate::tenant::Tenant {
                id: tenant_id,
                name: "Acme".to_string(),
                slug: "acme".to_string(),
                created: 0,
            })
            .await
            .unwrap();

        let resp = super::oidc_login(
            State(st),
            axum::extract::Query(super::LoginParams {
                org: Some("acme".to_string()),
            }),
            ReqHeaderMap::new(),
        )
        .await;
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
        assert!(
            resp.headers().get(axum::http::header::LOCATION).is_none(),
            "a tenant with no OIDC config of its own must never fall back to the global IdP"
        );
    }

    /// Slug-enumeration close (multi-tenancy P2d Task 4b): an unknown slug
    /// and a real tenant with no OIDC config of its own must return the
    /// exact same 404 body. Before this fix they carried distinct messages
    /// ("unknown organization" vs "organization has no identity provider
    /// configured"), letting an unauthenticated caller tell real slugs apart
    /// from made-up ones one probe at a time.
    #[tokio::test]
    async fn org_login_unknown_slug_and_unconfigured_tenant_return_identical_404_body() {
        let st = multi_tenant_state().await;
        let tenant_id = crate::tenant::TenantId(st.store.next_tenant_id().await.unwrap());
        st.store
            .put_tenant(&crate::tenant::Tenant {
                id: tenant_id,
                name: "Acme".to_string(),
                slug: "acme".to_string(),
                created: 0,
            })
            .await
            .unwrap();

        let unknown_resp = super::oidc_login(
            State(st.clone()),
            axum::extract::Query(super::LoginParams {
                org: Some("ghost-org".to_string()),
            }),
            ReqHeaderMap::new(),
        )
        .await;
        let unconfigured_resp = super::oidc_login(
            State(st),
            axum::extract::Query(super::LoginParams {
                org: Some("acme".to_string()),
            }),
            ReqHeaderMap::new(),
        )
        .await;

        assert_eq!(unknown_resp.status(), axum::http::StatusCode::NOT_FOUND);
        assert_eq!(
            unconfigured_resp.status(),
            axum::http::StatusCode::NOT_FOUND
        );
        let unknown_body = axum::body::to_bytes(unknown_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let unconfigured_body = axum::body::to_bytes(unconfigured_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            unknown_body, unconfigured_body,
            "unknown slug and unconfigured tenant must be indistinguishable"
        );
    }

    #[tokio::test]
    async fn org_login_requires_multi_tenant_mode() {
        let st = guard_state_with_oidc(None, false).await; // multi_tenant: false
        let resp = super::oidc_login(
            State(st),
            axum::extract::Query(super::LoginParams {
                org: Some("acme".to_string()),
            }),
            ReqHeaderMap::new(),
        )
        .await;
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn org_login_absent_is_the_unchanged_global_path() {
        // multi_tenant: true, but no `?org=` and no global OIDC configured:
        // behaves exactly like the pre-P2d global path (404, oidc not
        // configured), regardless of the cloud/OSS deployment mode.
        let st = multi_tenant_state().await;
        let resp = super::oidc_login(
            State(st),
            axum::extract::Query(super::LoginParams { org: None }),
            ReqHeaderMap::new(),
        )
        .await;
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn org_login_empty_string_is_treated_as_absent() {
        let st = multi_tenant_state().await;
        let resp = super::oidc_login(
            State(st),
            axum::extract::Query(super::LoginParams {
                org: Some(String::new()),
            }),
            ReqHeaderMap::new(),
        )
        .await;
        // Same outcome as `org: None`: falls to the global path (404 here,
        // since no global OIDC is configured), not treated as a slug lookup.
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
    }

    // Status-contract restore (multi-tenancy P2d Task 4b): before per-tenant
    // login existed, `oidc_callback` checked `st.oidc.is_none()` first,
    // unconditionally, so a request against a deployment with no global
    // OIDC configured was always 404 ("oidc not configured") — regardless
    // of `?error=`, cookie presence, `state`, or `code`. The P2d refactor
    // moved that check inside the `None`-tenant match arm, so it only fired
    // after the error/cookie/state/code checks had already returned their
    // own (401/400) status first. These two tests pin the contract back to
    // 404 for the two ways a request resolves to "no tenant, no global IdP".
    #[tokio::test]
    async fn callback_no_global_oidc_missing_cookie_is_404() {
        let st = guard_state_with_oidc(None, false).await;
        let resp = super::oidc_callback(
            State(st),
            axum::extract::Query(super::CallbackParams {
                code: Some("some-code".to_string()),
                state: Some("some-state".to_string()),
                error: None,
            }),
            ReqHeaderMap::new(),
        )
        .await;
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn callback_no_global_oidc_with_error_param_is_404_not_401() {
        // Even an IdP-supplied `?error=` must not preempt the "no OIDC
        // configured at all" 404 when there is no tenant to fall back on.
        let st = guard_state_with_oidc(None, false).await;
        let resp = super::oidc_callback(
            State(st),
            axum::extract::Query(super::CallbackParams {
                code: None,
                state: None,
                error: Some("access_denied".to_string()),
            }),
            ReqHeaderMap::new(),
        )
        .await;
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn callback_tenant_from_cookie_but_config_gone_is_400_not_global() {
        // The tenant signed into the cookie no longer has an OIDC config
        // (e.g. removed mid-flow, or a forged tenant id that happens to
        // exist but was never configured). This must be an explicit error,
        // never a fall-through to the global IdP.
        let st = multi_tenant_state().await;
        let tenant_id = crate::tenant::TenantId(st.store.next_tenant_id().await.unwrap());
        st.store
            .put_tenant(&crate::tenant::Tenant {
                id: tenant_id,
                name: "Acme".to_string(),
                slug: "acme".to_string(),
                created: 0,
            })
            .await
            .unwrap();
        let cookie_value =
            crate::oidc::sign_login_state(&st.signing_key, "st8", "verif", "nnc", Some(tenant_id));
        let mut headers = ReqHeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            format!("qk_login={cookie_value}").parse().unwrap(),
        );
        let resp = super::oidc_callback(
            State(st),
            axum::extract::Query(super::CallbackParams {
                code: Some("code".to_string()),
                state: Some("st8".to_string()),
                error: None,
            }),
            headers,
        )
        .await;
        assert_eq!(resp.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn callback_tampered_tenant_in_cookie_is_rejected() {
        // A cookie whose tenant field was swapped for a different tenant id
        // must fail the HMAC check entirely (verified at the `oidc.rs`
        // level), so no tenant can be trusted out of it — at the HTTP layer
        // that is indistinguishable from no cookie at all, which (per the
        // restored status-contract test above) is 404 here since this
        // deployment has no global OIDC configured either. Either way, the
        // swapped-in tenant is never authenticated into.
        let st = multi_tenant_state().await;
        let real_tenant = crate::tenant::TenantId(1);
        let cookie_value = crate::oidc::sign_login_state(
            &st.signing_key,
            "st8",
            "verif",
            "nnc",
            Some(real_tenant),
        );
        let tampered = cookie_value.replacen(".1.", ".2.", 1);
        assert_ne!(
            tampered, cookie_value,
            "sanity: tamper must actually change the value"
        );
        let mut headers = ReqHeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            format!("qk_login={tampered}").parse().unwrap(),
        );
        let resp = super::oidc_callback(
            State(st),
            axum::extract::Query(super::CallbackParams {
                code: Some("code".to_string()),
                state: Some("st8".to_string()),
                error: None,
            }),
            headers,
        )
        .await;
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
    }

    /// `claim_role` never grants `Role::Owner`, and its Admin/Viewer/Member
    /// mapping matches the `TenantOidcConfig`'s claim, end to end through
    /// `ensure_user_and_membership` with a real store (LMDB) — the same path
    /// `oidc_callback` drives for a per-tenant login. This is the decision
    /// logic the HTTP callback cannot exercise without a live IdP, tested
    /// directly instead.
    #[tokio::test]
    async fn tenant_login_membership_role_matches_claim_mapping() {
        let st = multi_tenant_state().await;
        let cfg = crate::oidc::TenantOidcConfig {
            tenant_id: crate::tenant::TenantId(1),
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
        let tenant = crate::tenant::TenantId(1);

        let admin_claims = serde_json::json!({ "groups": ["acme-admins"] });
        let role = crate::oidc::claim_role(&admin_claims, &cfg);
        assert_eq!(role, crate::tenant::Role::Admin);
        let uid = crate::oidc::ensure_user_and_membership(
            st.store.as_ref(),
            true,
            "sub-a",
            "a@acme.example",
            "A",
            &[],
            Some((tenant, role)),
        )
        .await
        .unwrap();
        let m = st.store.get_membership(uid, tenant).await.unwrap().unwrap();
        assert_eq!(m.role, crate::tenant::Role::Admin);

        let viewer_claims = serde_json::json!({ "groups": ["acme-viewers"] });
        let role = crate::oidc::claim_role(&viewer_claims, &cfg);
        assert_eq!(role, crate::tenant::Role::Viewer);

        let neither_claims = serde_json::json!({ "groups": ["nobody"] });
        let role = crate::oidc::claim_role(&neither_claims, &cfg);
        assert_eq!(role, crate::tenant::Role::Member);
        assert_ne!(role, crate::tenant::Role::Owner);
    }

    /// A login into tenant A's own OIDC creates a membership ONLY in A, never
    /// in any other tenant — same decision-logic level as
    /// `tenant_login_membership_role_matches_claim_mapping`, but asserting
    /// the negative: `ensure_user_and_membership` is only ever told about the
    /// login's own tenant, so `list_memberships_for_user` for that user must
    /// come back with exactly one entry, scoped to A.
    #[tokio::test]
    async fn tenant_login_creates_membership_only_in_the_login_tenant() {
        let st = multi_tenant_state().await;
        let tenant_a = crate::tenant::TenantId(1);
        let tenant_b = crate::tenant::TenantId(2);
        st.store
            .put_tenant(&crate::tenant::Tenant {
                id: tenant_a,
                name: "Acme".to_string(),
                slug: "acme".to_string(),
                created: 0,
            })
            .await
            .unwrap();
        st.store
            .put_tenant(&crate::tenant::Tenant {
                id: tenant_b,
                name: "Bravo".to_string(),
                slug: "bravo".to_string(),
                created: 0,
            })
            .await
            .unwrap();
        let cfg_a = crate::oidc::TenantOidcConfig {
            tenant_id: tenant_a,
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

        let claims = serde_json::json!({ "groups": ["acme-admins"] });
        let role = crate::oidc::claim_role(&claims, &cfg_a);
        let uid = crate::oidc::ensure_user_and_membership(
            st.store.as_ref(),
            true,
            "sub-cross-tenant",
            "x@acme.example",
            "X",
            &[],
            Some((tenant_a, role)),
        )
        .await
        .unwrap();

        let memberships = st.store.list_memberships_for_user(uid).await.unwrap();
        assert_eq!(
            memberships.len(),
            1,
            "the login into A must not create a membership anywhere else"
        );
        assert_eq!(memberships[0].tenant_id, tenant_a);
        assert!(
            st.store
                .get_membership(uid, tenant_b)
                .await
                .unwrap()
                .is_none(),
            "no membership must exist in tenant B from a login into tenant A"
        );
    }

    /// Security fix (P2d Task 5b, final-branch-review finding): a tenant's
    /// `Owner` logging back in through that tenant's own IdP must not be
    /// downgraded by whatever role the claim maps to. `claim_role` never
    /// produces `Owner`, so before this fix a second login by the sole Owner
    /// silently demoted them and left the tenant with no Owner at all
    /// (Owner-only operations become unreachable — an availability bug, not
    /// an escalation). `ensure_user_and_membership` must now read the
    /// existing membership first and keep `Owner` rather than overwrite it.
    #[tokio::test]
    async fn tenant_login_never_downgrades_an_existing_owner() {
        let st = multi_tenant_state().await;
        let tenant = crate::tenant::TenantId(1);
        st.store
            .put_tenant(&crate::tenant::Tenant {
                id: tenant,
                name: "Acme".to_string(),
                slug: "acme".to_string(),
                created: 0,
            })
            .await
            .unwrap();

        // Grant Owner the way a real workspace creation would (never through
        // `ensure_user_and_membership`/claim_role, which can't produce it).
        let uid = st.store.next_user_id().await.unwrap();
        st.store
            .put_user(&crate::tenant::User {
                id: uid,
                subject: "sub-owner".to_string(),
                email: "owner@acme.example".to_string(),
                display: "Owner".to_string(),
                created: 0,
            })
            .await
            .unwrap();
        st.store
            .put_membership(&crate::tenant::Membership {
                user_id: uid,
                tenant_id: tenant,
                role: crate::tenant::Role::Owner,
                created: 0,
            })
            .await
            .unwrap();

        // The IdP's admin-group claim maps to Admin, not Owner — as if the
        // Owner were removed from the admin group, or the tenant's IdP
        // config simply doesn't distinguish an Owner group at all.
        let cfg = crate::oidc::TenantOidcConfig {
            tenant_id: tenant,
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
        let claims = serde_json::json!({ "groups": ["acme-admins"] });
        let role = crate::oidc::claim_role(&claims, &cfg);
        assert_eq!(role, crate::tenant::Role::Admin);

        let uid2 = crate::oidc::ensure_user_and_membership(
            st.store.as_ref(),
            true,
            "sub-owner",
            "owner@acme.example",
            "Owner",
            &[],
            Some((tenant, role)),
        )
        .await
        .unwrap();
        assert_eq!(uid2, uid, "same subject must resolve to the same user");

        let m = st.store.get_membership(uid, tenant).await.unwrap().unwrap();
        assert_eq!(
            m.role,
            crate::tenant::Role::Owner,
            "the Owner's own login must not downgrade them via the claim"
        );
    }

    /// Counterpart to the Owner-preservation test above: a non-owner's
    /// membership must still follow the claim on every login, so a group
    /// change (Member promoted into the admin group) keeps taking effect.
    /// Only `Owner` is special-cased; this asserts the fix didn't freeze
    /// every role.
    #[tokio::test]
    async fn tenant_login_still_applies_claim_role_for_non_owners() {
        let st = multi_tenant_state().await;
        let tenant = crate::tenant::TenantId(1);
        st.store
            .put_tenant(&crate::tenant::Tenant {
                id: tenant,
                name: "Acme".to_string(),
                slug: "acme".to_string(),
                created: 0,
            })
            .await
            .unwrap();
        let cfg = crate::oidc::TenantOidcConfig {
            tenant_id: tenant,
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

        // First login: no admin-group claim yet, lands as Member (default).
        let member_claims = serde_json::json!({ "groups": ["nobody"] });
        let role = crate::oidc::claim_role(&member_claims, &cfg);
        assert_eq!(role, crate::tenant::Role::Member);
        let uid = crate::oidc::ensure_user_and_membership(
            st.store.as_ref(),
            true,
            "sub-member",
            "member@acme.example",
            "Member",
            &[],
            Some((tenant, role)),
        )
        .await
        .unwrap();
        let m = st.store.get_membership(uid, tenant).await.unwrap().unwrap();
        assert_eq!(m.role, crate::tenant::Role::Member);

        // Second login: now in the admin group — must be upgraded to Admin,
        // since a non-owner's role always tracks the claim.
        let admin_claims = serde_json::json!({ "groups": ["acme-admins"] });
        let role = crate::oidc::claim_role(&admin_claims, &cfg);
        assert_eq!(role, crate::tenant::Role::Admin);
        let uid2 = crate::oidc::ensure_user_and_membership(
            st.store.as_ref(),
            true,
            "sub-member",
            "member@acme.example",
            "Member",
            &[],
            Some((tenant, role)),
        )
        .await
        .unwrap();
        assert_eq!(uid2, uid);
        let m = st.store.get_membership(uid, tenant).await.unwrap().unwrap();
        assert_eq!(
            m.role,
            crate::tenant::Role::Admin,
            "a non-owner's role must follow the claim on every login"
        );
    }

    /// Brand-new user via per-tenant login still just gets the claim role —
    /// the Owner-preservation branch in `ensure_user_and_membership` must not
    /// change behavior when there is no prior membership to preserve.
    #[tokio::test]
    async fn tenant_login_new_user_gets_claim_role() {
        let st = multi_tenant_state().await;
        let tenant = crate::tenant::TenantId(1);
        st.store
            .put_tenant(&crate::tenant::Tenant {
                id: tenant,
                name: "Acme".to_string(),
                slug: "acme".to_string(),
                created: 0,
            })
            .await
            .unwrap();
        let cfg = crate::oidc::TenantOidcConfig {
            tenant_id: tenant,
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
        let claims = serde_json::json!({ "groups": ["acme-admins"] });
        let role = crate::oidc::claim_role(&claims, &cfg);
        let uid = crate::oidc::ensure_user_and_membership(
            st.store.as_ref(),
            true,
            "sub-brand-new",
            "new@acme.example",
            "New",
            &[],
            Some((tenant, role)),
        )
        .await
        .unwrap();
        let m = st.store.get_membership(uid, tenant).await.unwrap().unwrap();
        assert_eq!(m.role, crate::tenant::Role::Admin);
    }

    /// Required-group gate (multi-tenancy P2d Task 4b), driven at the same
    /// level as `tenant_login_membership_role_matches_claim_mapping`:
    /// `passes_required_group` is the decision `oidc_callback` must check
    /// BEFORE `ensure_user_and_membership`, so this asserts the gate denies
    /// (and nothing is written) rather than merely returning `false`.
    /// Without `required_value` set, the gate stays open — unchanged from
    /// before this task.
    #[tokio::test]
    async fn required_group_gate_open_when_unconfigured() {
        let st = multi_tenant_state().await;
        let cfg = crate::oidc::TenantOidcConfig {
            tenant_id: crate::tenant::TenantId(1),
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
        let tenant = crate::tenant::TenantId(1);

        // Any authenticated user, in none of the groups, still passes the
        // gate (though `claim_role` still gives them only the open Member
        // default) — the open-by-default contract this task must not break.
        let claims = serde_json::json!({ "groups": ["nobody-in-particular"] });
        assert!(crate::oidc::passes_required_group(&claims, &cfg));
        let role = crate::oidc::claim_role(&claims, &cfg);
        assert_eq!(role, crate::tenant::Role::Member);
        let uid = crate::oidc::ensure_user_and_membership(
            st.store.as_ref(),
            true,
            "sub-open",
            "open@acme.example",
            "Open",
            &[],
            Some((tenant, role)),
        )
        .await
        .unwrap();
        let m = st.store.get_membership(uid, tenant).await.unwrap().unwrap();
        assert_eq!(m.role, crate::tenant::Role::Member);
    }

    /// With `required_value` set: a user in none of admin/readonly/required
    /// is denied by the gate BEFORE any membership is considered; a member of
    /// the required group passes (and gets the open Member role, since they
    /// match neither admin_value nor readonly_value); a member of the admin
    /// group passes the gate too (their claim already satisfies it) and
    /// keeps the Admin role.
    #[tokio::test]
    async fn required_group_gate_closed_when_configured() {
        let st = multi_tenant_state().await;
        let cfg = crate::oidc::TenantOidcConfig {
            tenant_id: crate::tenant::TenantId(1),
            issuer: "https://idp.acme.example".into(),
            client_id: "acme".into(),
            client_secret: "s".into(),
            scopes: vec!["openid".into()],
            admin_claim: "groups".into(),
            admin_value: "acme-admins".into(),
            readonly_value: "acme-viewers".into(),
            required_value: Some("acme-contractors".to_string()),
            post_login_url: None,
        };
        let tenant = crate::tenant::TenantId(1);

        // Neither admin, readonly, nor the required group: the gate denies,
        // and (mirroring exactly what `oidc_callback` does on this branch)
        // `ensure_user_and_membership` is never reached — no user, no
        // membership, for an outsider who never should have gotten in.
        let outsider_claims = serde_json::json!({ "groups": ["random"] });
        assert!(!crate::oidc::passes_required_group(&outsider_claims, &cfg));
        assert!(
            st.store
                .get_user_by_subject("sub-outsider")
                .await
                .unwrap()
                .is_none(),
            "a caller denied by the required-group gate must never get a user record"
        );

        // The required group itself: gate passes, and (since they match
        // neither admin_value nor readonly_value) `claim_role` still gives
        // them only Member — the gate and the role are independent checks.
        let required_claims = serde_json::json!({ "groups": ["acme-contractors"] });
        assert!(crate::oidc::passes_required_group(&required_claims, &cfg));
        let role = crate::oidc::claim_role(&required_claims, &cfg);
        assert_eq!(role, crate::tenant::Role::Member);
        let uid = crate::oidc::ensure_user_and_membership(
            st.store.as_ref(),
            true,
            "sub-required",
            "required@acme.example",
            "Required",
            &[],
            Some((tenant, role)),
        )
        .await
        .unwrap();
        let m = st.store.get_membership(uid, tenant).await.unwrap().unwrap();
        assert_eq!(m.role, crate::tenant::Role::Member);

        // The admin group: gate passes (their claim already satisfies it
        // independent of `required_value`), and the role is still Admin.
        let admin_claims = serde_json::json!({ "groups": ["acme-admins"] });
        assert!(crate::oidc::passes_required_group(&admin_claims, &cfg));
        let admin_role = crate::oidc::claim_role(&admin_claims, &cfg);
        assert_eq!(admin_role, crate::tenant::Role::Admin);
        let admin_uid = crate::oidc::ensure_user_and_membership(
            st.store.as_ref(),
            true,
            "sub-admin",
            "admin@acme.example",
            "Admin",
            &[],
            Some((tenant, admin_role)),
        )
        .await
        .unwrap();
        let admin_m = st
            .store
            .get_membership(admin_uid, tenant)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(admin_m.role, crate::tenant::Role::Admin);
    }

    /// `admin_guard` returns the resolved `Principal` on every success path
    /// while keeping the status contract (401/403) byte for byte. The
    /// integration admin-auth tests guard the full 401/403/404/429/503 matrix
    /// end to end; this asserts the in-process Principal contents the HTTP
    /// surface cannot observe in P1b (tenant is always the default).
    #[tokio::test]
    async fn admin_guard_resolves_principal_per_credential() {
        use super::admin_guard;
        use crate::auth::{hash_token, ApiToken, Scope};
        use crate::tenant::DEFAULT_TENANT;
        use axum::http::{HeaderMap as GuardHeaders, StatusCode};

        let st = guard_state(Some("secret")).await;

        // 1) env admin token present + provided -> Full principal, default tenant.
        let mut h = GuardHeaders::new();
        h.insert("x-admin-token", "secret".parse().unwrap());
        let p = admin_guard(&st, &h, Scope::Full)
            .await
            .expect("env admin token authorizes");
        assert_eq!(p.tenant, DEFAULT_TENANT);
        assert_eq!(p.user_id, None);
        assert_eq!(p.scopes, vec![Scope::Full]);

        // 3) no credential, env token configured -> 401 (contract preserved).
        assert_eq!(
            admin_guard(&st, &GuardHeaders::new(), Scope::Full)
                .await
                .unwrap_err(),
            StatusCode::UNAUTHORIZED
        );

        // A stored API token scoped to [LinksRead] on the default tenant.
        let plaintext = "qtok_principal_resolution_test";
        let token = ApiToken {
            id: 1,
            name: "t".into(),
            token_hash: hash_token(plaintext),
            scopes: vec![Scope::LinksRead],
            rate_limit_per_min: None,
            created: 0,
            tenant_id: DEFAULT_TENANT,
        };
        st.store
            .put_api_token(DEFAULT_TENANT, &token)
            .await
            .unwrap();
        let mut ht = GuardHeaders::new();
        ht.insert("x-admin-token", plaintext.parse().unwrap());

        // 2) covering API token -> Principal carries the token's tenant + scopes.
        let p = admin_guard(&st, &ht, Scope::LinksRead)
            .await
            .expect("api token covers LinksRead");
        assert_eq!(p.tenant, DEFAULT_TENANT);
        assert_eq!(p.user_id, None);
        assert_eq!(p.scopes, vec![Scope::LinksRead]);

        // 4) valid-but-insufficient token -> 403 (contract preserved).
        assert_eq!(
            admin_guard(&st, &ht, Scope::Full).await.unwrap_err(),
            StatusCode::FORBIDDEN
        );
    }

    /// OSS session with EMPTY `session.scopes` must still yield 403, not 401.
    /// The OIDC-session branch in `admin_guard` unconditionally sets
    /// `saw_insufficient` after a failed covering check (byte-for-byte with
    /// the original behavior) precisely so this case falls through to the
    /// 403 tail instead of `not_found_status` (401). The OIDC callback
    /// currently rejects empty-scope logins, so this session shape doesn't
    /// arise in practice today — but the guard's own status contract must
    /// not depend on that invariant holding in another function.
    #[tokio::test]
    async fn admin_guard_oss_empty_scope_session_is_forbidden_not_unauthorized() {
        use super::admin_guard;
        use crate::auth::{hash_token, Scope, Session};
        use axum::http::{HeaderMap as GuardHeaders, StatusCode};

        let st = guard_state_with_oidc(None, true).await;
        assert!(!st.multi_tenant);

        let raw = "oss_empty_scope_session_test";
        let session = Session {
            token_hash: hash_token(raw),
            subject: "sub".into(),
            display: "display".into(),
            scopes: Vec::new(),
            created: 0,
            expires: u64::MAX,
            tenant_id: crate::tenant::DEFAULT_TENANT,
            user_id: 7,
        };
        st.store
            .put_session(crate::tenant::DEFAULT_TENANT, &session)
            .await
            .unwrap();

        let mut headers = GuardHeaders::new();
        headers.insert(
            axum::http::header::COOKIE,
            format!("qk_session={raw}").parse().unwrap(),
        );

        assert_eq!(
            admin_guard(&st, &headers, Scope::LinksRead)
                .await
                .unwrap_err(),
            StatusCode::FORBIDDEN
        );
    }

    /// `session_user_id` must gate on `st.oidc_configured`, same as
    /// `admin_guard`'s session branch: a leftover session cookie must stop
    /// resolving a user the instant OIDC is disabled, even though the
    /// session row itself is still valid in the store.
    #[tokio::test]
    async fn session_user_id_none_when_oidc_not_configured() {
        use super::session_user_id;
        use crate::auth::{hash_token, Session};

        let raw = "session_gate_test_token";
        let session = Session {
            token_hash: hash_token(raw),
            subject: "sub".into(),
            display: "display".into(),
            scopes: Vec::new(),
            created: 0,
            expires: u64::MAX,
            tenant_id: crate::tenant::DEFAULT_TENANT,
            user_id: 42,
        };

        // OIDC disabled: even a store-valid session cookie must resolve to
        // nothing, matching admin_guard's session-branch gate.
        let st_off = guard_state_with_oidc(None, false).await;
        st_off
            .store
            .put_session(crate::tenant::DEFAULT_TENANT, &session)
            .await
            .unwrap();
        let mut headers = ReqHeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            format!("qk_session={raw}").parse().unwrap(),
        );
        assert_eq!(session_user_id(&st_off, &headers).await, None);

        // OIDC enabled + same valid session -> resolves the user_id.
        let st_on = guard_state_with_oidc(None, true).await;
        st_on
            .store
            .put_session(crate::tenant::DEFAULT_TENANT, &session)
            .await
            .unwrap();
        assert_eq!(session_user_id(&st_on, &headers).await, Some(42));
    }

    /// P2a Task 3: `create_link_core` must write under the `tenant` PARAM, not
    /// `DEFAULT_TENANT`. Exercises both branches (numeric id and custom alias)
    /// against a store keyed by tenant, so a regression to the old hardcode
    /// would make the link/alias invisible under the passed tenant (and
    /// visible under `DEFAULT_TENANT` instead).
    #[tokio::test]
    async fn create_link_core_numeric_writes_under_the_passed_tenant() {
        let st = guard_state(None).await;
        let tenant = crate::tenant::TenantId(7);
        let headers = ReqHeaderMap::new();

        let code = create_link_core(
            &st,
            tenant,
            SHARED_DOMAIN_ID,
            "https://example.com/numeric",
            None,
            None,
            Vec::new(),
            None,
            Vec::new(),
            Vec::new(),
            None,
            None,
            None,
            None,
            None,
            &headers,
        )
        .await
        .expect("create succeeds");
        let permuted = crate::codec::from_base62(&code).expect("numeric code decodes");
        let id = crate::permute::decode(permuted, st.key);

        assert!(
            st.store.get_link(tenant, id).await.unwrap().is_some(),
            "the link must be visible under the passed tenant"
        );
        assert!(
            st.store
                .get_link(crate::tenant::DEFAULT_TENANT, id)
                .await
                .unwrap()
                .is_none(),
            "the link must NOT be visible under DEFAULT_TENANT"
        );
    }

    /// P3 Task 2: the alias namespace moved from per-tenant to per-domain, so
    /// a written alias now resolves regardless of which tenant asks (any
    /// tenant creating through the shared namespace lands on the same
    /// `SHARED_DOMAIN_ID`). Cross-domain isolation itself is exercised by the
    /// PG-gated `alias_namespace_is_per_domain` in `tests/domains_it.rs`.
    #[tokio::test]
    async fn create_link_core_alias_resolves_via_the_shared_domain() {
        let st = guard_state(None).await;
        let tenant = crate::tenant::TenantId(7);
        let headers = ReqHeaderMap::new();

        let code = create_link_core(
            &st,
            tenant,
            SHARED_DOMAIN_ID,
            "https://example.com/alias",
            Some("my-alias"),
            None,
            Vec::new(),
            None,
            Vec::new(),
            Vec::new(),
            None,
            None,
            None,
            None,
            None,
            &headers,
        )
        .await
        .expect("create succeeds");
        assert_eq!(code, "my-alias");

        assert!(
            st.store
                .get_alias(SHARED_DOMAIN_ID, "my-alias")
                .await
                .unwrap()
                .is_some(),
            "the alias must resolve in the shared domain namespace"
        );
    }

    /// `resolve_code`'s alias branch resolves through whichever domain id is
    /// passed in; the shared domain (`SHARED_DOMAIN_ID`) is one such domain,
    /// used regardless of which tenant created the alias (alias isolation is
    /// by domain, not by tenant; see `resolve_code`'s doc comment).
    #[tokio::test]
    async fn resolve_code_resolves_alias_via_the_shared_domain() {
        let st = guard_state(None).await;
        let tenant = crate::tenant::TenantId(9);
        let rec = Record {
            url: "https://example.com".into(),
            expiry: None,
            created: 0,
            tags: Vec::new(),
            max_visits: None,
            rules: Vec::new(),
            variants: Vec::new(),
            app_ios: None,
            app_android: None,
            folder: None,
            fallback_url: None,
            password_hash: None,
            tenant_id: crate::tenant::DEFAULT_TENANT,
        };
        st.store
            .put_alias_and_link(tenant, SHARED_DOMAIN_ID, "foo", 5, &rec)
            .await
            .unwrap();

        assert_eq!(
            resolve_code(&st, SHARED_DOMAIN_ID, "foo").await.unwrap(),
            Some(5),
            "resolve_code must resolve the alias via the shared domain"
        );
    }

    /// `resolve_for_admin`'s alias branch resolves through the shared domain
    /// namespace, mirroring `resolve_code`.
    #[tokio::test]
    async fn resolve_for_admin_resolves_alias_via_the_shared_domain() {
        let st = guard_state(None).await;
        let tenant = crate::tenant::TenantId(11);
        let rec = Record {
            url: "https://example.com".into(),
            expiry: None,
            created: 0,
            tags: Vec::new(),
            max_visits: None,
            rules: Vec::new(),
            variants: Vec::new(),
            app_ios: None,
            app_android: None,
            folder: None,
            fallback_url: None,
            password_hash: None,
            tenant_id: crate::tenant::DEFAULT_TENANT,
        };
        st.store
            .put_alias_and_link(tenant, SHARED_DOMAIN_ID, "bar", 6, &rec)
            .await
            .unwrap();

        assert_eq!(
            resolve_for_admin(&st, tenant, "bar").await.unwrap(),
            Some((6, Some("bar".to_string()))),
            "resolve_for_admin must resolve the alias via the shared domain"
        );
        assert_eq!(
            resolve_for_admin(&st, crate::tenant::DEFAULT_TENANT, "bar")
                .await
                .unwrap(),
            Some((6, Some("bar".to_string()))),
            "resolve_for_admin must resolve the alias regardless of the passed tenant"
        );
    }

    const IPHONE_UA: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X)";
    const IPAD_UA: &str = "Mozilla/5.0 (iPad; CPU OS 17_0 like Mac OS X)";
    const IPOD_UA: &str = "Mozilla/5.0 (iPod touch; CPU iPhone OS 17_0 like Mac OS X)";
    const ANDROID_UA: &str = "Mozilla/5.0 (Linux; Android 14; Pixel 8)";
    const DESKTOP_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64)";

    #[test]
    fn normalize_max_visits_zero_or_absent_is_unlimited() {
        assert_eq!(normalize_max_visits(None), None);
        assert_eq!(normalize_max_visits(Some(0)), None);
    }

    #[test]
    fn normalize_max_visits_positive_is_some() {
        assert_eq!(normalize_max_visits(Some(1)), Some(1));
        assert_eq!(normalize_max_visits(Some(42)), Some(42));
    }

    #[test]
    fn fbclid_from_query_present() {
        assert_eq!(
            fbclid_from_query(Some("a=1&fbclid=abc123&b=2")),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn fbclid_from_query_absent() {
        assert_eq!(fbclid_from_query(Some("a=1&b=2")), None);
        assert_eq!(fbclid_from_query(None), None);
    }

    #[test]
    fn fbclid_from_query_urlencoded_value_is_decoded() {
        assert_eq!(
            fbclid_from_query(Some("fbclid=IwAR%2Bx%20y")),
            Some("IwAR+x y".to_string())
        );
    }

    #[test]
    fn fbclid_from_query_empty_is_none() {
        assert_eq!(fbclid_from_query(Some("")), None);
        assert_eq!(fbclid_from_query(Some("fbclid=")), None);
    }

    #[test]
    fn classify_platform_detects_apple_devices() {
        assert_eq!(classify_platform(Some(IPHONE_UA)), Platform::Ios);
        assert_eq!(classify_platform(Some(IPAD_UA)), Platform::Ios);
        assert_eq!(classify_platform(Some(IPOD_UA)), Platform::Ios);
    }

    #[test]
    fn classify_platform_detects_android() {
        assert_eq!(classify_platform(Some(ANDROID_UA)), Platform::Android);
    }

    #[test]
    fn classify_platform_falls_back_to_other() {
        assert_eq!(classify_platform(Some(DESKTOP_UA)), Platform::Other);
        assert_eq!(classify_platform(Some("")), Platform::Other);
        assert_eq!(classify_platform(None), Platform::Other);
    }

    #[test]
    fn app_destination_returns_platform_match() {
        let r = rec(
            Some("https://apps.apple.com/x"),
            Some("https://play.google.com/y"),
        );
        assert_eq!(
            app_destination(&r, Some(IPHONE_UA)),
            Some("https://apps.apple.com/x")
        );
        assert_eq!(
            app_destination(&r, Some(ANDROID_UA)),
            Some("https://play.google.com/y")
        );
    }

    #[test]
    fn app_destination_falls_back_when_platform_unset() {
        let r = rec(Some("https://apps.apple.com/x"), None);
        assert_eq!(app_destination(&r, Some(ANDROID_UA)), None);
        assert_eq!(app_destination(&r, Some(DESKTOP_UA)), None);
    }

    #[test]
    fn app_destination_none_when_no_fields() {
        let r = rec(None, None);
        assert_eq!(app_destination(&r, Some(IPHONE_UA)), None);
        assert_eq!(app_destination(&r, Some(ANDROID_UA)), None);
    }

    #[test]
    fn parse_cors_origins_splits_and_trims() {
        assert_eq!(parse_cors_origins(None), Vec::<String>::new());
        assert_eq!(parse_cors_origins(Some("".into())), Vec::<String>::new());
        assert_eq!(
            parse_cors_origins(Some(" https://a.com , https://b.com ".into())),
            vec!["https://a.com".to_string(), "https://b.com".to_string()]
        );
    }

    #[test]
    fn cache_control_without_expiry_uses_default() {
        assert_eq!(cache_control_for(None, 1_000), "public, max-age=86400");
    }

    #[test]
    fn cache_control_with_future_expiry_uses_difference() {
        let now = 1_000;
        assert_eq!(
            cache_control_for(Some(now + 100), now),
            "public, max-age=100"
        );
    }

    #[test]
    fn cache_control_with_distant_future_expiry_caps_at_default() {
        let now = 1_000;
        assert_eq!(
            cache_control_for(Some(now + 999_999), now),
            "public, max-age=86400"
        );
    }

    #[test]
    fn cache_control_with_past_expiry_is_no_store() {
        let now = 1_000;
        assert_eq!(cache_control_for(Some(now - 1), now), "no-store");
    }

    #[test]
    fn access_log_line_is_valid_json_with_expected_fields() {
        let line = access_log_line("GET", "/abc", 302, 0.4139);
        let v: serde_json::Value =
            serde_json::from_str(&line).expect("access_log_line should produce valid JSON");
        assert_eq!(v["method"], "GET");
        assert_eq!(v["path"], "/abc");
        assert_eq!(v["status"], 302);
        assert_eq!(v["latency_ms"], 0.414);
    }

    #[test]
    fn access_log_line_escapes_special_characters_in_path() {
        let path = "/a\"b\\c";
        let line = access_log_line("GET", path, 200, 1.0);
        let v: serde_json::Value = serde_json::from_str(&line)
            .expect("access_log_line should escape correctly and remain valid JSON");
        assert_eq!(v["path"], path);
    }

    /// Captured request: headers (lowercased names) + raw body. Mirrors the
    /// mock server in `webhooks::delivery`'s test module.
    struct Captured {
        headers: std::collections::HashMap<String, String>,
        body: String,
    }

    struct ServerState {
        captured: Mutex<Vec<Captured>>,
    }

    async fn handler(
        State(state): State<std::sync::Arc<ServerState>>,
        headers: ReqHeaderMap,
        body: Bytes,
    ) -> axum::http::StatusCode {
        let mut map = std::collections::HashMap::new();
        for (k, v) in headers.iter() {
            map.insert(
                k.as_str().to_ascii_lowercase(),
                v.to_str().unwrap().to_string(),
            );
        }
        state.captured.lock().unwrap().push(Captured {
            headers: map,
            body: String::from_utf8(body.to_vec()).unwrap(),
        });
        axum::http::StatusCode::OK
    }

    /// Spins a local server capturing every POST it receives. Returns the
    /// base URL and the shared state to inspect.
    async fn spawn_test_server() -> (String, std::sync::Arc<ServerState>) {
        let state = std::sync::Arc::new(ServerState {
            captured: Mutex::new(Vec::new()),
        });
        let app = TestRouter::new()
            .route("/hook", any(handler))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}/hook"), state)
    }

    fn sub(url: &str, secret: &str, kind: SubscriptionKind) -> WebhookSubscription {
        WebhookSubscription {
            id: 1,
            url: url.to_string(),
            events: vec![EventType::LinkCreated],
            secret: secret.to_string(),
            active: true,
            created: 0,
            kind,
        }
    }

    /// Regression for review Task 1 of #6: a Slack-kind subscription's
    /// test-send must receive the same channel-formatted, unsigned payload a
    /// real delivery would send — not the signed Generic envelope the
    /// endpoint used to always build. This is exercised through
    /// `send_test_event_guarded` (the SSRF-guard-injectable core of
    /// `admin_webhooks_test`) since the guard's real predicate always blocks
    /// the loopback address a local test server binds to (see that
    /// function's doc comment).
    #[tokio::test]
    async fn test_send_on_slack_sub_is_unsigned_channel_payload() {
        let (url, state) = spawn_test_server().await;
        let slack_sub = sub(&url, "", SubscriptionKind::Slack);

        let resp = send_test_event_guarded(&slack_sub, |_| false).await;
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        let captured = state.captured.lock().unwrap();
        assert_eq!(captured.len(), 1);
        let req = &captured[0];
        let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
        assert!(body["text"].as_str().unwrap().contains("TEST0000"));
        assert!(!req.headers.contains_key("webhook-signature"));
        assert!(!req.headers.contains_key("webhook-id"));
        assert!(!req.headers.contains_key("webhook-timestamp"));
    }

    /// Counterpart: a Generic subscription's test-send must remain the
    /// signed Standard Webhooks envelope, body verbatim.
    #[tokio::test]
    async fn test_send_on_generic_sub_stays_signed() {
        let (url, state) = spawn_test_server().await;
        let secret = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw".to_string();
        let generic_sub = sub(&url, &secret, SubscriptionKind::Generic);

        let resp = send_test_event_guarded(&generic_sub, |_| false).await;
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        let captured = state.captured.lock().unwrap();
        assert_eq!(captured.len(), 1);
        let req = &captured[0];
        let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
        assert_eq!(body["data"]["code"], "TEST0000");
        let msg_id = req.headers.get("webhook-id").expect("webhook-id header");
        let ts: i64 = req
            .headers
            .get("webhook-timestamp")
            .expect("webhook-timestamp header")
            .parse()
            .unwrap();
        let sig = req
            .headers
            .get("webhook-signature")
            .expect("webhook-signature header");
        let expected = crate::webhooks::sign(&secret, msg_id, ts, &req.body).unwrap();
        assert_eq!(sig, &expected);
    }
}
