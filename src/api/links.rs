use super::*;

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
pub(crate) fn normalize_max_visits(raw: Option<u32>) -> Option<u32> {
    raw.filter(|&n| n > 0)
}

/// Maximum number of geo/device rules a single link may carry.
pub(crate) const MAX_RULES: usize = 20;

/// Validates and normalizes rules for `create`/`admin_link_patch`: caps the
/// count, normalizes country codes to uppercase and device values to the
/// canonical `Mobile`/`Desktop`/`Other` set, and runs each rule's `to`
/// through the SAME SSRF guard as the link's main `url` (a rule
/// destination must not smuggle an internal/self host).
pub(crate) async fn validate_rules(
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

pub(crate) fn is_valid_url(u: &str) -> bool {
    u.starts_with("http://") || u.starts_with("https://")
}

pub(crate) const DEFAULT_MAX_AGE: u64 = 86400;
/// Default page size for admin listing/search endpoints when `limit` is not provided.
pub(crate) const DEFAULT_PAGE_LIMIT: usize = 50;
/// Maximum page size accepted for admin listing/search endpoints (clamp ceiling).
pub(crate) const MAX_PAGE_LIMIT: usize = 500;
/// Maximum number of webhook subscriptions a deployment may register.
pub(crate) const MAX_WEBHOOK_SUBSCRIPTIONS: usize = 50;
/// Timeout for the synchronous one-shot delivery used by the "test" endpoint.
pub(crate) const WEBHOOK_TEST_TIMEOUT_SECS: u64 = 5;
/// Default header carrying the real client IP behind a proxy (Cloudflare).
/// Overridable via `QUARK_REAL_IP_HEADER` at startup (see `main.rs`).
pub const DEFAULT_REAL_IP_HEADER: &str = "cf-connecting-ip";
/// Request timeout for the outbound `reqwest` client built by `reqwest_client`.
pub(crate) const HTTP_CLIENT_TIMEOUT_SECS: u64 = 10;

/// A random id embedded in an outbound event payload's `id` field.
/// Distinct from the `webhook-id` header the delivery worker assigns per
/// attempt (see `webhooks::delivery::deliver_one`): this one identifies the
/// event as recorded at emission time, before it is queued for delivery.
pub(crate) fn generate_event_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("system RNG must be available");
    let hex = crate::hex(&bytes);
    format!("evt_{hex}")
}

/// A stable per-click id (`clk_` + 16 random bytes hex), generated once when a
/// redirect captures a click. Mirrors `generate_event_id` / the webhook
/// `generate_msg_id`. Carried on the `ClickEvent` through the analytics channel
/// so a future at-least-once retry sends the same id, which Meta uses to
/// deduplicate the conversion.
pub(crate) fn generate_click_id() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        return String::new();
    }
    let hex = crate::hex(&bytes);
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
pub(crate) fn webhook_event_payload(
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
pub(crate) fn validate_webhook_url(url: &str) -> Result<(), (StatusCode, &'static str)> {
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
pub(crate) fn mask_secret(secret: &str) -> String {
    if secret.is_empty() {
        String::new()
    } else {
        "whsec_••••".to_string()
    }
}

/// Cap on the number of A/B variants a single link may have.
pub(crate) const MAX_VARIANTS: usize = 10;

/// Validates a set of A/B variants against the same rules as the main `url`:
/// count cap, `is_valid_url`, SSRF guard (`is_blocked_target`), and a minimum
/// weight of 1. Shared by `create` and `admin_link_patch` so the two paths can
/// never drift out of sync on SSRF coverage.
pub(crate) async fn validate_variants(
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
pub(crate) const WELLKNOWN_NAMES: [&str; 2] = ["apple-app-site-association", "assetlinks.json"];
/// Maximum accepted body size for a well-known document (64 KiB).
pub(crate) const WELLKNOWN_MAX: usize = 65536;

/// Computes the Cache-Control header value for a redirect response,
/// respecting the link's TTL: never caches past expiry. Pure function,
/// a TDD target.
pub(crate) fn cache_control_for(expiry: Option<u64>, now: u64) -> String {
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
pub(crate) async fn require_admin_for_create(
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
        let canonical_code = st.encode_code(id);
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
            tenant_id: tenant,
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
    let code = st.encode_code(id);
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
        tenant_id: tenant,
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
pub(crate) fn create_error_response(err: CreateError) -> Response {
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
pub(crate) fn create_error_reason(err: &CreateError) -> &'static str {
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
pub(crate) async fn default_domain_id(st: &AppState, tenant: crate::tenant::TenantId) -> u64 {
    if !st.multi_tenant || tenant == crate::tenant::DEFAULT_TENANT {
        return SHARED_DOMAIN_ID;
    }
    // Primary link domain override (LUC-86): a verified custom domain the tenant
    // marked primary wins over the auto subdomain, so new links bind to it.
    if let Ok(Some(pid)) = st.store.get_primary_domain_id(tenant).await {
        if let Ok(Some(d)) = st.store.get_domain(tenant, pid).await {
            if d.status == crate::domain::DomainStatus::Verified {
                return d.id;
            }
        }
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

/// The host the panel should show/copy for a tenant's short links: the verified
/// primary custom domain if set (LUC-86), else the auto `<slug>.<suffix>`
/// subdomain, else the shared `public_host`. `None` only when nothing is
/// configured (OSS with no public host). Exposed on `/admin/me` so the panel
/// builds the copy URL without listing every domain.
pub(crate) async fn primary_link_host(
    st: &AppState,
    tenant: crate::tenant::TenantId,
) -> Option<String> {
    if st.multi_tenant && tenant != crate::tenant::DEFAULT_TENANT {
        if let Ok(Some(pid)) = st.store.get_primary_domain_id(tenant).await {
            if let Ok(Some(d)) = st.store.get_domain(tenant, pid).await {
                if d.status == crate::domain::DomainStatus::Verified {
                    return Some(d.host);
                }
            }
        }
        if let Some(suffix) = st.tenant_domain_suffix.as_deref() {
            if let Ok(Some(t)) = st.store.get_tenant(tenant).await {
                return Some(subdomain_host(&t.slug, suffix));
            }
        }
    }
    st.public_host.clone()
}

pub(crate) async fn create(
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
pub(crate) struct ImportFailure {
    index: usize,
    url: String,
    reason: String,
}

#[derive(Serialize)]
pub(crate) struct ImportSummary {
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
pub(crate) async fn admin_import(
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
pub(crate) fn client_ip(
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
pub(crate) enum Platform {
    Ios,
    Android,
    Other,
}

/// Classifies a click by User-Agent. Apple device tokens win over Android;
/// anything else (desktop, bots, missing header) is `Other`. Case-sensitive
/// substring match on the raw UA: these vendor tokens are stable.
pub(crate) fn classify_platform(ua: Option<&str>) -> Platform {
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
pub(crate) fn app_destination<'a>(rec: &'a Record, ua: Option<&str>) -> Option<&'a str> {
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
pub(crate) async fn is_blocked_target(host: &str, headers: &HeaderMap, st: &AppState) -> bool {
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
pub(crate) async fn app_destination_ok(
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
pub(crate) async fn resolve_code(
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
pub(crate) async fn resolve_host_route(
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
pub(crate) fn fbclid_from_query(raw: Option<&str>) -> Option<String> {
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
pub(crate) fn expired_response(fallback: Option<&str>) -> Response {
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
pub(crate) fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|kv| {
        let (k, v) = kv.trim().split_once('=')?;
        (k == name).then_some(v)
    })
}

/// Whether the request carries a valid, unexpired unlock cookie for `code`
/// (the link's canonical code) under its current `password_hash`. `key` is the
/// dedicated 32-byte signing secret.
pub(crate) fn is_unlocked(
    headers: &HeaderMap,
    key: &[u8],
    code: &str,
    password_hash: &str,
    now: u64,
) -> bool {
    match cookie_value(headers, &format!("qk_pw_{code}")) {
        Some(v) => crate::password::unlock_token_valid(v, key, code, password_hash, now),
        None => false,
    }
}

/// Whether the original client request arrived over HTTPS, per `X-Forwarded-Proto`
/// (quark runs behind a TLS-terminating proxy/CDN). Absent → treated as plain
/// HTTP so the `Secure` cookie attribute is not set on local/dev HTTP.
pub(crate) fn request_is_https(headers: &HeaderMap) -> bool {
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
pub(crate) fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Extracts a field from an `application/x-www-form-urlencoded` body.
pub(crate) fn form_field(body: &Bytes, name: &str) -> Option<String> {
    url::form_urlencoded::parse(body)
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
}

/// Renders the self-contained password interstitial. No external assets (inline
/// CSS only) so it works on any deployment. Bilingual by a simple
/// `Accept-Language` sniff; `error` shows a generic "wrong password" message.
pub(crate) fn interstitial_html(code: &str, query: Option<&str>, pt: bool, error: bool) -> String {
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
pub(crate) fn interstitial_response(
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
pub(crate) async fn unlock(
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
    let canonical = st.encode_code(id);
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

pub(crate) async fn redirect(
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
                            tenant_id: rec.tenant_id,
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
                let canonical = st.encode_code(id);
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
            // Global Privacy Control (GPC): honored by default, no config
            // flag. `Sec-GPC: 1` suppresses analytics capture and conversion
            // forwarding for this click (both fed by the same
            // `analytics_tx` send below); it does not affect the redirect,
            // `bump_visits`/`max_visits` enforcement, or the first-party
            // `link.clicked` operator webhook (gated separately below).
            let gpc = headers
                .get("sec-gpc")
                .and_then(|v| v.to_str().ok())
                .map(|v| v.trim() == "1")
                .unwrap_or(false);
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
                    tenant_id: rec.tenant_id,
                });
            }

            // GPC (`sec-gpc` header, read above) suppresses this send: the
            // same channel feeds both analytics capture and conversion
            // forwarding, so gating it here covers both per the opt-out.
            if !gpc {
                let _ = st.analytics_tx.try_send(ev);
            }

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

pub(crate) async fn stats(
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
pub(crate) async fn admin_stats(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let p = match admin_guard(&st, &headers, Scope::Analytics).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    match st.sink.stats_for_tenant(p.tenant.0).await {
        Ok(agg) => Json(agg).into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}
