use crate::abuse::{extract_host, is_internal_host};
use crate::analytics::{device_from_ua, AnalyticsSink, ClickEvent};
use crate::auth::{generate_token, hash_token, ApiToken, Scope};
use crate::cache::Cache;
use crate::pixel::{PixelConfig, PixelCredentials, Provider};
use crate::store::{
    matched_rule_index, normalize_tags, Record, Rule, RuleField, Store, StoreError,
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
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;
use tower_http::cors::{Any, CorsLayer};

pub struct AppState {
    pub cache: Cache,
    pub store: Arc<dyn Store>,
    pub key: u64,
    pub analytics_tx: tokio::sync::mpsc::Sender<ClickEvent>,
    pub sink: Arc<dyn AnalyticsSink>,
    pub admin_token: Option<String>,
    pub ratelimiter: crate::abuse::ratelimit::RateLimiter,
    pub blocklist: crate::abuse::blocklist::Blocklist,
    pub block_private: bool,
    pub public_host: Option<String>,
    pub real_ip_header: String,
    pub webhooks: Arc<WebhookDispatcher>,
}

#[derive(Deserialize)]
pub struct CreateReq {
    url: String,
    alias: Option<String>,
    ttl: Option<u64>,
    tags: Option<Vec<String>>,
    max_visits: Option<u32>,
    rules: Option<Vec<Rule>>,
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
/// through the SAME SSRF/blocklist guards as the link's main `url` (a rule
/// destination must not smuggle an internal/blocked host).
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
        if st.blocklist.is_blocked(&host, now()).await {
            return Err((StatusCode::FORBIDDEN, "blocked rule destination").into_response());
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
fn require_admin_for_create(st: &AppState, headers: &HeaderMap) -> Result<(), StatusCode> {
    match st.admin_token.as_deref() {
        None => Ok(()),
        Some(expected) => {
            let provided = headers
                .get("x-admin-token")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
                Ok(())
            } else {
                Err(StatusCode::UNAUTHORIZED)
            }
        }
    }
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
/// validates the URL, runs the blocklist/anti-loop guards, computes expiry
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
    if st.blocklist.is_blocked(&host, now()).await {
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
    };

    if let Some(alias) = alias {
        if codec::from_base62(alias).is_some() {
            return Err(CreateError::AliasCollision);
        }
        let id = match st.store.next_id().await {
            Ok(id) => id,
            Err(StoreError::IdSpaceExhausted) => return Err(CreateError::IdExhausted),
            Err(_) => return Err(CreateError::Backend),
        };
        match st.store.put_alias_and_link(alias, id, &rec).await {
            Ok(true) => {}
            Ok(false) => return Err(CreateError::AliasInUse),
            Err(_) => return Err(CreateError::Backend),
        };
        let canonical_code = codec::to_base62(permute::encode(id, st.key));
        st.webhooks.emit(WebhookEvent {
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
        });
        return Ok(alias.to_string());
    }

    let id = match st.store.next_id().await {
        Ok(id) => id,
        Err(StoreError::IdSpaceExhausted) => return Err(CreateError::IdExhausted),
        Err(_) => return Err(CreateError::Backend),
    };
    if id > permute::MAX_ID {
        return Err(CreateError::IdExhausted);
    }
    if st.store.put_link(id, &rec).await.is_err() {
        return Err(CreateError::Backend);
    }
    let code = codec::to_base62(permute::encode(id, st.key));
    st.webhooks.emit(WebhookEvent {
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
    });
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
    if let Err(status) = require_admin_for_create(&st, &headers) {
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
    match create_link_core(
        &st,
        &req.url,
        req.alias.as_deref(),
        req.ttl,
        normalize_tags(req.tags.clone().unwrap_or_default()),
        normalize_max_visits(req.max_visits),
        rules,
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
/// validation and blocklist rules match `POST /` exactly. Always admin-gated
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

/// Resolves a URL code into an id: first tries a numeric code (base62 in the
/// domain); if not, treats it as an alias in the store. `Ok(Some(id))` resolved,
/// `Ok(None)` doesn't exist, `Err` backend failure. Each handler maps these
/// cases to its own HTTP response (the redirect attaches Cache-Control on 404).
async fn resolve_code(st: &AppState, code: &str) -> Result<Option<u64>, StoreError> {
    match codec::from_base62(code) {
        Some(c) if c <= permute::MAX_ID => Ok(Some(permute::decode(c, st.key))),
        _ => st.store.get_alias(code).await,
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
        Ok(Some(rec)) => {
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
                    return (
                        StatusCode::GONE,
                        [(header::CACHE_CONTROL, "no-store".to_string())],
                        "expired link",
                    )
                        .into_response();
                }
            }
            if let Some(max) = rec.max_visits {
                let n = match st.store.bump_visits(id).await {
                    Ok(n) => n,
                    Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
                };
                if n > max as u64 {
                    return (
                        StatusCode::GONE,
                        [(header::CACHE_CONTROL, "no-store".to_string())],
                        "expired link",
                    )
                        .into_response();
                }
            }
            let cache_control = cache_control_for(rec.expiry, now);

            let country = headers
                .get("cf-ipcountry")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let user_agent = headers
                .get(header::USER_AGENT)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());

            // Zero-clone hot path: only the rule-match branch allocates. When
            // `rec.rules` is empty (every pre-existing link, the common
            // case), `rec.url` is moved straight into the LOCATION header.
            let dest: String =
                match matched_rule_index(&rec.rules, country.as_deref(), user_agent.as_deref()) {
                    Some(i) => rec.rules[i].to.clone(),
                    None => rec.url.clone(),
                };

            let ev = ClickEvent {
                id,
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
            };
            // Gate check first (cheap: one atomic load) so the payload build
            // — which reads `ev`'s fields — happens before `ev` is moved
            // into `try_send` below. No subscriber -> no extra work beyond
            // the load, same as before this fix.
            if st.webhooks.clicked_subscribed.load(Ordering::Relaxed) {
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
                    (header::LOCATION, dest),
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
    if let Err(status) = admin_guard(&st, &headers, Scope::Analytics).await {
        return status.into_response();
    }
    let id = match resolve_code(&st, &code).await {
        Ok(Some(id)) => id,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    match st.store.get_link(id).await {
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

#[derive(Deserialize)]
struct BlocklistReq {
    domain: String,
}

/// Authorizes an admin request against a required `Scope`: `Ok(())` if
/// authorized; `Err(status)` otherwise. Returns `StatusCode` (not `Response`)
/// to stay `Copy`/small — avoids clippy's `result_large_err` lint, which
/// would trigger with `Response` in the `Err`.
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
) -> Result<(), StatusCode> {
    let provided = headers
        .get("x-admin-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if let Some(expected) = st.admin_token.as_deref() {
        if constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
            return Ok(());
        }
    }

    let not_found_status = if st.admin_token.is_some() {
        StatusCode::UNAUTHORIZED
    } else {
        StatusCode::NOT_FOUND
    };

    if provided.is_empty() {
        return Err(not_found_status);
    }

    let hash = hash_token(provided);
    let token = match st.store.get_api_token_by_hash(&hash).await {
        Ok(Some(t)) => t,
        Ok(None) => return Err(not_found_status),
        Err(_) => return Err(StatusCode::SERVICE_UNAVAILABLE),
    };

    if !token.scopes.iter().any(|s| s.covers(required)) {
        return Err(StatusCode::FORBIDDEN);
    }

    if let Some(limit) = token.rate_limit_per_min {
        let key = format!("tok:{}", token.id);
        if !st.ratelimiter.check_with_limit(&key, now(), limit).await {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
    }

    Ok(())
}

async fn blocklist_get(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(status) = admin_guard(&st, &headers, Scope::Blocklist).await {
        return status.into_response();
    }
    match st.store.list_blocked_domains().await {
        Ok(domains) => Json(serde_json::json!({ "domains": domains })).into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

async fn blocklist_add(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(status) = admin_guard(&st, &headers, Scope::Blocklist).await {
        return status.into_response();
    }
    let req: BlocklistReq = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid json").into_response(),
    };
    if st.store.add_blocked_domain(&req.domain).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    st.blocklist.invalidate().await;
    StatusCode::OK.into_response()
}

async fn blocklist_delete(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(status) = admin_guard(&st, &headers, Scope::Blocklist).await {
        return status.into_response();
    }
    let req: BlocklistReq = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid json").into_response(),
    };
    if st.store.remove_blocked_domain(&req.domain).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    st.blocklist.invalidate().await;
    StatusCode::OK.into_response()
}

#[derive(Deserialize)]
struct ListParams {
    after: Option<u64>,
    limit: Option<usize>,
    q: Option<String>,
    tag: Option<String>,
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
}

async fn admin_links_list(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(p): Query<ListParams>,
) -> Response {
    if let Err(status) = admin_guard(&st, &headers, Scope::LinksRead).await {
        return status.into_response();
    }
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
    let links = match q {
        Some(term) => match st.store.search_links(term, p.after, limit, tag).await {
            Ok(l) => l,
            Err(StoreError::Unsupported) => return StatusCode::NOT_IMPLEMENTED.into_response(),
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        },
        None => match st.store.list_links(p.after, limit, tag).await {
            Ok(l) => l,
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        },
    };
    let alias_map: std::collections::HashMap<u64, String> = match st.store.list_aliases().await {
        Ok(pairs) => pairs.into_iter().map(|(a, id)| (id, a)).collect(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let next_after = if links.len() == limit {
        links.last().map(|(id, _)| *id)
    } else {
        None
    };
    let mut rows: Vec<LinkRow> = Vec::with_capacity(links.len());
    for (id, rec) in links {
        let visits = match st.store.visits(id).await {
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
        });
    }
    Json(serde_json::json!({ "links": rows, "next_after": next_after })).into_response()
}

/// `GET /admin/tags`: the distinct set of tags across all links, for the
/// panel's filter control.
async fn admin_tags_list(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(status) = admin_guard(&st, &headers, Scope::LinksRead).await {
        return status.into_response();
    }
    match st.store.list_tags().await {
        Ok(tags) => Json(serde_json::json!({ "tags": tags })).into_response(),
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
        _ => match st.store.get_alias(code).await? {
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
    if let Err(status) = admin_guard(&st, &headers, Scope::LinksWrite).await {
        return status.into_response();
    }
    let (id, alias) = match resolve_for_admin(&st, &code).await {
        Ok(Some(v)) => v,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let rec = match st.store.get_link(id).await {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    if st.store.delete_link(id).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if let Some(a) = &alias {
        let _ = st.store.delete_alias(a).await;
    }
    st.cache.invalidate(id).await;
    let canonical_code = codec::to_base62(permute::encode(id, st.key));
    st.webhooks.emit(WebhookEvent {
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
    });
    StatusCode::OK.into_response()
}

async fn admin_link_patch(
    State(st): State<Arc<AppState>>,
    Path(code): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(status) = admin_guard(&st, &headers, Scope::LinksWrite).await {
        return status.into_response();
    }
    let (id, alias) = match resolve_for_admin(&st, &code).await {
        Ok(Some(v)) => v,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let mut rec = match st.store.get_link(id).await {
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
        if st.blocklist.is_blocked(&host, now()).await {
            return (StatusCode::FORBIDDEN, "blocked destination").into_response();
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
    if st.store.put_link(id, &rec).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    st.cache.invalidate(id).await;
    let canonical_code = codec::to_base62(permute::encode(id, st.key));
    st.webhooks.emit(WebhookEvent {
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
    });
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
    if let Err(status) = admin_guard(&st, &headers, Scope::Webhooks).await {
        return status.into_response();
    }
    match st.store.list_webhooks().await {
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
    if let Err(status) = admin_guard(&st, &headers, Scope::Analytics).await {
        return status.into_response();
    }
    match st.store.list_pixels().await {
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
    if let Err(status) = admin_guard(&st, &headers, Scope::Full).await {
        return status.into_response();
    }
    match st.store.list_api_tokens().await {
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
    if let Err(status) = admin_guard(&st, &headers, Scope::Webhooks).await {
        return status.into_response();
    }
    if let Err((status, msg)) = validate_webhook_url(&req.url) {
        return (status, msg).into_response();
    }
    let count = match st.store.list_webhooks().await {
        Ok(subs) => subs.len(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    if count >= MAX_WEBHOOK_SUBSCRIPTIONS {
        return (StatusCode::BAD_REQUEST, "webhook subscription cap reached").into_response();
    }
    let id = match st.store.next_webhook_id().await {
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
    if st.store.put_webhook(&sub).await.is_err() {
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
    if let Err(status) = admin_guard(&st, &headers, Scope::Webhooks).await {
        return status.into_response();
    }
    let mut sub = match st.store.get_webhook(id).await {
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
    if st.store.put_webhook(&sub).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    StatusCode::OK.into_response()
}

async fn admin_webhooks_delete(
    State(st): State<Arc<AppState>>,
    Path(id): Path<u64>,
    headers: HeaderMap,
) -> Response {
    if let Err(status) = admin_guard(&st, &headers, Scope::Webhooks).await {
        return status.into_response();
    }
    match st.store.delete_webhook(id).await {
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
    if let Err(status) = admin_guard(&st, &headers, Scope::Full).await {
        return status.into_response();
    }
    let req: CreateTokenReq = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid json").into_response(),
    };
    let existing = match st.store.list_api_tokens().await {
        Ok(t) => t,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    if existing.len() >= MAX_API_TOKENS {
        return (StatusCode::BAD_REQUEST, "token cap reached").into_response();
    }
    let id = match st.store.next_api_token_id().await {
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
    };
    if st.store.put_api_token(&token).await.is_err() {
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
    if let Err(status) = admin_guard(&st, &headers, Scope::Full).await {
        return status.into_response();
    }
    match st.store.delete_api_token(id).await {
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
    if let Err(status) = admin_guard(&st, &headers, Scope::Analytics).await {
        return status.into_response();
    }
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
    let existing = match st.store.list_pixels().await {
        Ok(p) => p,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    if existing.len() >= PIXELS_CAP {
        return (StatusCode::BAD_REQUEST, "pixel config limit reached (20)").into_response();
    }
    let id = match st.store.next_pixel_id().await {
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
    if st.store.put_pixel(&config).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    (StatusCode::CREATED, Json(to_pixel_row(&config))).into_response()
}

async fn admin_pixels_delete(
    State(st): State<Arc<AppState>>,
    Path(id): Path<u64>,
    headers: HeaderMap,
) -> Response {
    if let Err(status) = admin_guard(&st, &headers, Scope::Analytics).await {
        return status.into_response();
    }
    match st.store.delete_pixel(id).await {
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
    if let Err(status) = admin_guard(&st, &headers, Scope::Webhooks).await {
        return status.into_response();
    }
    let sub = match st.store.get_webhook(id).await {
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
    let req = match webhooks::delivery::build_outgoing_request(sub, &ev) {
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
        .route("/:code", get(redirect))
        .route("/:code/stats", get(stats))
        .route(
            "/admin/blocklist",
            get(blocklist_get)
                .post(blocklist_add)
                .delete(blocklist_delete),
        )
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
        .route("/admin/tags", get(admin_tags_list))
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
        .with_state(state);

    let app = if origins.is_empty() {
        app
    } else {
        let list: Vec<axum::http::HeaderValue> =
            origins.iter().filter_map(|o| o.parse().ok()).collect();
        let cors = CorsLayer::new()
            .allow_origin(list)
            .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE])
            .allow_headers(Any);
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
        access_log_line, cache_control_for, fbclid_from_query, normalize_max_visits,
        parse_cors_origins, send_test_event_guarded, EventType, SubscriptionKind,
        WebhookSubscription,
    };
    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::HeaderMap as ReqHeaderMap;
    use axum::routing::any;
    use axum::Router as TestRouter;
    use std::sync::Mutex;
    use tokio::net::TcpListener;

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
