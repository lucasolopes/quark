use crate::abuse::{extract_host, is_internal_host};
use crate::analytics::{device_from_ua, AnalyticsSink, ClickEvent};
use crate::cache::Cache;
use crate::store::{Record, Store, StoreError};
use crate::webhooks::delivery::WebhookDispatcher;
use crate::webhooks::{self, EventType, WebhookEvent, WebhookSubscription};
use crate::{codec, now, permute};
use axum::body::Bytes;
use axum::extract::{ConnectInfo, Path, Query, Request, State};
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
/// once, at creation time.
fn mask_secret(_secret: &str) -> String {
    "whsec_••••".to_string()
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
    if !is_valid_url(&req.url) {
        return (StatusCode::BAD_REQUEST, "invalid url").into_response();
    }
    let Some(host) = extract_host(&req.url) else {
        return (StatusCode::BAD_REQUEST, "url without host").into_response();
    };
    if st.block_private && is_blocked_target(&host, &headers, &st) {
        return (StatusCode::FORBIDDEN, "destination not allowed").into_response();
    }
    if st.blocklist.is_blocked(&host, now()).await {
        return (StatusCode::FORBIDDEN, "blocked destination").into_response();
    }

    let expiry = match req.ttl {
        Some(t) => match now().checked_add(t) {
            Some(e) => Some(e),
            None => return (StatusCode::BAD_REQUEST, "invalid ttl").into_response(),
        },
        None => None,
    };
    let rec = Record {
        url: req.url.clone(),
        expiry,
        created: now(),
    };

    if let Some(alias) = req.alias {
        if codec::from_base62(&alias).is_some() {
            return (
                StatusCode::BAD_REQUEST,
                "alias collides with the numeric code space",
            )
                .into_response();
        }
        let id = match st.store.next_id().await {
            Ok(id) => id,
            Err(StoreError::IdSpaceExhausted) => {
                return (StatusCode::INSUFFICIENT_STORAGE, "id space exhausted").into_response()
            }
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        };
        match st.store.put_alias_and_link(&alias, id, &rec).await {
            Ok(true) => {}
            Ok(false) => return (StatusCode::CONFLICT, "alias in use").into_response(),
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        };
        let canonical_code = codec::to_base62(permute::encode(id, st.key));
        st.webhooks.emit(WebhookEvent {
            event_type: EventType::LinkCreated,
            body: webhook_event_payload(
                EventType::LinkCreated,
                &canonical_code,
                &rec.url,
                Some(&alias),
                rec.expiry,
                rec.created,
                None,
            ),
        });
        return Json(CreateResp {
            code: alias,
            url: req.url,
        })
        .into_response();
    }

    let id = match st.store.next_id().await {
        Ok(id) => id,
        Err(StoreError::IdSpaceExhausted) => {
            return (StatusCode::INSUFFICIENT_STORAGE, "id space exhausted").into_response()
        }
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    if id > permute::MAX_ID {
        return (StatusCode::INSUFFICIENT_STORAGE, "id space exhausted").into_response();
    }
    if st.store.put_link(id, &rec).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
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
    Json(CreateResp { code, url: req.url }).into_response()
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

async fn redirect(
    State(st): State<Arc<AppState>>,
    Path(code): Path<String>,
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
            let cache_control = cache_control_for(rec.expiry, now);

            let ev = ClickEvent {
                id,
                ts: now,
                referer: headers
                    .get(header::REFERER)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string()),
                country: headers
                    .get("cf-ipcountry")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string()),
                user_agent: headers
                    .get(header::USER_AGENT)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string()),
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
                    (header::LOCATION, rec.url),
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
    let Some(expected) = st.admin_token.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let provided = headers
        .get("x-admin-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
        return StatusCode::UNAUTHORIZED.into_response();
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

/// Authorizes an admin request: `Ok(())` if the token matches; `Err(status)` otherwise.
/// No token configured -> 404 (endpoint disabled); wrong token -> 401.
/// Returns `StatusCode` (not `Response`) to stay `Copy`/small — avoids clippy's
/// `result_large_err` lint, which would trigger with `Response` in the `Err`.
fn admin_guard(st: &AppState, headers: &HeaderMap) -> Result<(), StatusCode> {
    let Some(expected) = st.admin_token.as_deref() else {
        return Err(StatusCode::NOT_FOUND);
    };
    let provided = headers
        .get("x-admin-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(())
}

async fn blocklist_get(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(status) = admin_guard(&st, &headers) {
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
    if let Err(status) = admin_guard(&st, &headers) {
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
    if let Err(status) = admin_guard(&st, &headers) {
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
}

async fn admin_links_list(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(p): Query<ListParams>,
) -> Response {
    if let Err(status) = admin_guard(&st, &headers) {
        return status.into_response();
    }
    let limit = p
        .limit
        .unwrap_or(DEFAULT_PAGE_LIMIT)
        .clamp(1, MAX_PAGE_LIMIT);
    let q = p.q.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let links = match q {
        Some(term) => match st.store.search_links(term, p.after, limit).await {
            Ok(l) => l,
            Err(StoreError::Unsupported) => return StatusCode::NOT_IMPLEMENTED.into_response(),
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        },
        None => match st.store.list_links(p.after, limit).await {
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
    let rows: Vec<LinkRow> = links
        .into_iter()
        .map(|(id, rec)| LinkRow {
            id,
            code: codec::to_base62(permute::encode(id, st.key)),
            alias: alias_map.get(&id).cloned(),
            url: rec.url,
            expiry: rec.expiry,
            created: rec.created,
        })
        .collect();
    Json(serde_json::json!({ "links": rows, "next_after": next_after })).into_response()
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
    if let Err(status) = admin_guard(&st, &headers) {
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
    if let Err(status) = admin_guard(&st, &headers) {
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
}

#[derive(Deserialize)]
struct WebhookPatchReq {
    url: Option<String>,
    events: Option<Vec<EventType>>,
    active: Option<bool>,
}

#[derive(Serialize)]
struct WebhookRow {
    id: u64,
    url: String,
    events: Vec<EventType>,
    active: bool,
    created: u64,
    secret_masked: String,
}

async fn admin_webhooks_list(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(status) = admin_guard(&st, &headers) {
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
                })
                .collect();
            Json(serde_json::json!({ "webhooks": rows })).into_response()
        }
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

async fn admin_webhooks_create(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<WebhookCreateReq>,
) -> Response {
    if let Err(status) = admin_guard(&st, &headers) {
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
    let secret = webhooks::generate_secret();
    let sub = WebhookSubscription {
        id,
        url: req.url,
        events: req.events,
        secret: secret.clone(),
        active: req.active.unwrap_or(true),
        created: now(),
    };
    if st.store.put_webhook(&sub).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    (
        StatusCode::CREATED,
        Json(serde_json::json!({ "id": id, "secret": secret })),
    )
        .into_response()
}

async fn admin_webhooks_patch(
    State(st): State<Arc<AppState>>,
    Path(id): Path<u64>,
    headers: HeaderMap,
    Json(req): Json<WebhookPatchReq>,
) -> Response {
    if let Err(status) = admin_guard(&st, &headers) {
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
    if let Err(status) = admin_guard(&st, &headers) {
        return status.into_response();
    }
    match st.store.delete_webhook(id).await {
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
    if let Err(status) = admin_guard(&st, &headers) {
        return status.into_response();
    }
    let sub = match st.store.get_webhook(id).await {
        Ok(Some(s)) => s,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let body = webhook_event_payload(
        EventType::LinkCreated,
        "TEST0000",
        "https://example.com/test",
        None,
        None,
        now(),
        None,
    );
    let msg_id = generate_event_id();
    let ts = now() as i64;
    let signature = match webhooks::sign(&sub.secret, &msg_id, ts, &body) {
        Ok(s) => s,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(WEBHOOK_TEST_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(c) => c,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let result = client
        .post(&sub.url)
        .header("webhook-id", &msg_id)
        .header("webhook-timestamp", ts.to_string())
        .header("webhook-signature", &signature)
        .body(body)
        .send()
        .await;
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
    use super::{access_log_line, cache_control_for, parse_cors_origins};

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
}
