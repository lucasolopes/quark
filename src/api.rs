use crate::abuse::{extract_host, is_internal_host};
use crate::analytics::{device_from_ua, AnalyticsSink, ClickEvent};
use crate::auth::{generate_token, hash_token, ApiToken, Scope};
use crate::cache::Cache;
use crate::pixel::{PixelConfig, PixelCredentials, Provider};
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
        if st.block_private && is_blocked_target(&host, headers, st) {
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
        if st.block_private && is_blocked_target(&host, headers, st) {
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
async fn require_admin_for_create(st: &AppState, headers: &HeaderMap) -> Result<(), StatusCode> {
    // Open shortener: create stays public ONLY when no auth mechanism is
    // configured. When either a token or OIDC is set, create requires a
    // credential covering LinksWrite (env token / API token / OIDC session),
    // reusing the same authorization as every other write.
    if st.admin_token.is_none() && !st.oidc_configured {
        return Ok(());
    }
    admin_guard(st, headers, Scope::LinksWrite).await.map(|_| ())
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
    if st.block_private && is_blocked_target(&host, headers, st) {
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
    };

    if let Some(alias) = alias {
        if codec::from_base62(alias).is_some() {
            return Err(CreateError::AliasCollision);
        }
        let id = match st.store.next_id(crate::tenant::DEFAULT_TENANT).await {
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
        let rows = st.webhooks.lifecycle_deliveries(&ev).await;
        match st.store.put_alias_and_link_tx(crate::tenant::DEFAULT_TENANT, alias, id, &rec, &rows).await {
            Ok(true) => {}
            Ok(false) => return Err(CreateError::AliasInUse),
            Err(_) => return Err(CreateError::Backend),
        };
        st.webhooks.emit_if_in_memory(ev);
        return Ok(alias.to_string());
    }

    let id = match st.store.next_id(crate::tenant::DEFAULT_TENANT).await {
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
    let rows = st.webhooks.lifecycle_deliveries(&ev).await;
    if st.store.put_link_tx(crate::tenant::DEFAULT_TENANT, id, &rec, &rows).await.is_err() {
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

async fn create(
    State(st): State<Arc<AppState>>,
    conn: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    Json(req): Json<CreateReq>,
) -> Response {
    if let Err(status) = require_admin_for_create(&st, &headers).await {
        return status.into_response();
    }
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
    match create_link_core(
        &st,
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
    if let Err(status) = admin_guard(&st, &headers, Scope::LinksWrite).await {
        return status.into_response();
    }
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

    let mut imported = 0usize;
    let mut failed = Vec::new();
    for (index, row) in rows.into_iter().enumerate() {
        match create_link_core(
            &st,
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

/// Built-in guard: internal network destination, or a loop back to quark's own host.
fn is_blocked_target(host: &str, headers: &HeaderMap, st: &AppState) -> bool {
    if is_internal_host(host) {
        return true;
    }
    let self_host = st.public_host.clone().or_else(|| {
        headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .map(|h| h.split(':').next().unwrap_or(h).to_ascii_lowercase())
    });
    matches!(self_host, Some(sh) if sh == host)
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
    if st.block_private && is_blocked_target(&host, headers, st) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

/// Resolves a URL code into an id: first tries a numeric code (base62 in the
/// domain); if not, treats it as an alias in the store. `Ok(Some(id))` resolved,
/// `Ok(None)` doesn't exist, `Err` backend failure. Each handler maps these
/// cases to its own HTTP response (the redirect attaches Cache-Control on 404).
async fn resolve_code(st: &AppState, code: &str) -> Result<Option<u64>, StoreError> {
    match codec::from_base62(code) {
        Some(c) if c <= permute::MAX_ID => Ok(Some(permute::decode(c, st.key))),
        _ => st.store.get_alias(crate::tenant::DEFAULT_TENANT, code).await,
    }
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
    let id = match resolve_code(&st, &code).await {
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
    let rec = match st.cache.get(id).await {
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
    let id = match resolve_code(&st, &code).await {
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
    match st.cache.get(id).await {
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
                let n = match st.store.bump_visits(crate::tenant::DEFAULT_TENANT, id).await {
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
    let id = match resolve_code(&st, &code).await {
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
                    if session.scopes.iter().any(|s| s.covers(required)) {
                        return Ok(Principal {
                            tenant: session.tenant_id,
                            user_id: Some(session.user_id),
                            scopes: session.scopes.clone(),
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

/// `GET /admin/login`: start the OIDC Authorization Code + PKCE flow, stashing
/// the state/verifier/nonce in a short-lived signed cookie and redirecting to
/// the IdP.
async fn oidc_login(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let Some(oidc) = st.oidc.as_ref() else {
        return (StatusCode::NOT_FOUND, "oidc not configured").into_response();
    };
    let state = crate::oidc::random_token();
    let nonce = crate::oidc::random_token();
    let (verifier, challenge) = crate::oidc::pkce_pair();
    let url = oidc.authorize_url(&state, &nonce, &challenge);
    let value = crate::oidc::sign_login_state(&st.signing_key, &state, &verifier, &nonce);
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
async fn oidc_callback(
    State(st): State<Arc<AppState>>,
    Query(params): Query<CallbackParams>,
    headers: HeaderMap,
) -> Response {
    let Some(oidc) = st.oidc.as_ref() else {
        return (StatusCode::NOT_FOUND, "oidc not configured").into_response();
    };
    if params.error.is_some() {
        return (StatusCode::UNAUTHORIZED, "login was denied at the provider").into_response();
    }
    let login = cookie_value(&headers, LOGIN_COOKIE)
        .and_then(|c| crate::oidc::verify_login_state(&st.signing_key, c));
    let Some((state, verifier, nonce)) = login else {
        return (StatusCode::BAD_REQUEST, "missing or invalid login state").into_response();
    };
    // CSRF: the state echoed by the IdP must match the one we signed.
    if params.state.as_deref() != Some(state.as_str()) {
        return (StatusCode::BAD_REQUEST, "state mismatch").into_response();
    }
    let Some(code) = params.code else {
        return (StatusCode::BAD_REQUEST, "missing code").into_response();
    };
    let id_token = match oidc.exchange_code(&code, &verifier).await {
        Ok(t) => t,
        Err(_) => return (StatusCode::BAD_GATEWAY, "token exchange failed").into_response(),
    };
    let claims = match oidc.verify(&id_token, &nonce).await {
        Ok(c) => c,
        Err(_) => return (StatusCode::UNAUTHORIZED, "invalid id_token").into_response(),
    };
    let scopes = crate::oidc::map_scopes(&claims.raw, &oidc.config);
    if scopes.is_empty() {
        return (StatusCode::FORBIDDEN, "your account has no quark access").into_response();
    }
    let raw = generate_token();
    let now = now();
    let session = crate::auth::Session {
        token_hash: hash_token(&raw),
        subject: claims.subject,
        display: claims.display,
        scopes,
        created: now,
        expires: now + SESSION_TTL_SECS,
        // Real user_id (linking to a `users`/`memberships` row) lands in a
        // later multi-tenancy task; OSS/P1b callers stay on the default tenant.
        tenant_id: crate::tenant::DEFAULT_TENANT,
        user_id: 0,
    };
    if st.store.put_session(crate::tenant::DEFAULT_TENANT, &session).await.is_err() {
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
    let dest = oidc.config.post_login_url.clone();
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
            return Json(serde_json::json!({
                "authenticated": true,
                "display": session.display,
                "scopes": session.scopes,
                "oidc_enabled": oidc_enabled,
            }))
            .into_response();
        }
    }
    Json(serde_json::json!({ "authenticated": false, "oidc_enabled": oidc_enabled }))
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
    if let Err(status) = admin_guard(&st, &headers, Scope::Full).await {
        return status.into_response();
    }
    let Some(cfg) = st.sheets.as_ref() else {
        return sheets_off_status(&st).into_response();
    };
    // The random `state` goes to Google in the URL; a signed copy is ALSO stored
    // in a short-lived HttpOnly cookie. The callback requires both to match, so
    // the state is bound to THIS browser and cannot be replayed by an attacker who
    // merely observes a leaked `state` value (login-CSRF). This is the same
    // double-submit binding the OIDC login flow uses.
    let state = crate::oidc::random_token();
    let signed = crate::oidc::sign_login_state(&st.signing_key, &state, "", "");
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
    let cookie_state = cookie_value(&headers, SHEETS_STATE_COOKIE)
        .and_then(|c| crate::oidc::verify_login_state(&st.signing_key, c))
        .map(|(state, _, _)| state);
    let matches = match (cookie_state.as_deref(), params.state.as_deref()) {
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
    if st.store.put_sheets_connection(crate::tenant::DEFAULT_TENANT, &conn).await.is_err() {
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
    if st.store.put_sheets_connection(p.tenant, &conn).await.is_err() {
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
            None => match st.store.list_links(prin.tenant, p.after, limit, tag, folder).await {
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
    let alias_map: std::collections::HashMap<u64, String> = match st.store.list_aliases(prin.tenant).await {
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
async fn resolve_for_admin(
    st: &AppState,
    code: &str,
) -> Result<Option<(u64, Option<String>)>, StoreError> {
    match codec::from_base62(code) {
        Some(c) if c <= permute::MAX_ID => Ok(Some((permute::decode(c, st.key), None))),
        _ => match st.store.get_alias(crate::tenant::DEFAULT_TENANT, code).await? {
            Some(id) => Ok(Some((id, Some(code.to_string())))),
            None => Ok(None),
        },
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
    let (id, alias) = match resolve_for_admin(&st, &code).await {
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
    let rows = st.webhooks.lifecycle_deliveries(&ev).await;
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
    let (id, alias) = match resolve_for_admin(&st, &code).await {
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
        if st.block_private && is_blocked_target(&host, &headers, &st) {
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
    let rows = st.webhooks.lifecycle_deliveries(&ev).await;
    if st.store.put_link_tx(p.tenant, id, &rec, &rows).await.is_err() {
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
/// `Some(body)` -> 200 verbatim; `None` -> 404; store error -> 503.
async fn serve_wellknown(st: &AppState, name: &str) -> Response {
    match st.store.get_wellknown(crate::tenant::DEFAULT_TENANT, name).await {
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

async fn wellknown_aasa(State(st): State<Arc<AppState>>) -> Response {
    serve_wellknown(&st, "apple-app-site-association").await
}

async fn wellknown_assetlinks(State(st): State<Arc<AppState>>) -> Response {
    serve_wellknown(&st, "assetlinks.json").await
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
        access_log_line, app_destination, cache_control_for, classify_platform, fbclid_from_query,
        normalize_max_visits, parse_cors_origins, send_test_event_guarded, EventType, Platform,
        SubscriptionKind, WebhookSubscription,
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
        }
    }

    /// Minimal `AppState` for exercising `admin_guard` directly: LMDB-backed
    /// store (so API tokens can be inserted), no OIDC/sheets, rate limiter
    /// disabled. `admin_token` sets (or clears) the env break-glass token.
    async fn guard_state(admin_token: Option<&str>) -> Arc<super::AppState> {
        let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
        let (store, sink) = crate::store::open_backends(dir.path()).await.unwrap();
        let cache = crate::cache::Cache::new(store.clone(), 1000, None);
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
            signing_key: [0u8; 32],
            analytics_tx,
            sink,
            admin_token: admin_token.map(str::to_string),
            ratelimiter: crate::abuse::ratelimit::RateLimiter::disabled(),
            block_private: true,
            public_host: None,
            real_ip_header: "cf-connecting-ip".to_string(),
            webhooks,
        })
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
        st.store.put_api_token(DEFAULT_TENANT, &token).await.unwrap();
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
