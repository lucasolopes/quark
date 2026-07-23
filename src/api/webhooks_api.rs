use super::*;

#[derive(Deserialize)]
pub(crate) struct WebhookCreateReq {
    url: String,
    events: Vec<EventType>,
    active: Option<bool>,
    #[serde(default)]
    kind: SubscriptionKind,
}

#[derive(Deserialize)]
pub(crate) struct WebhookPatchReq {
    url: Option<String>,
    events: Option<Vec<EventType>>,
    active: Option<bool>,
    kind: Option<SubscriptionKind>,
}

#[derive(Serialize)]
pub(crate) struct WebhookRow {
    id: u64,
    url: String,
    events: Vec<EventType>,
    active: bool,
    created: u64,
    secret_masked: String,
    kind: SubscriptionKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
}

pub(crate) async fn admin_webhooks_list(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
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
                    label: s.label,
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
pub(crate) async fn serve_wellknown(st: &AppState, name: &str, headers: &HeaderMap) -> Response {
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
pub(crate) const PIXELS_CAP: usize = 20;
/// Placeholder shown instead of a secret credential in `GET /admin/pixels`.
/// The raw value is never sent back once stored.
pub(crate) const MASKED_SECRET: &str = "\u{2022}\u{2022}\u{2022}\u{2022}";

#[derive(Deserialize)]
pub(crate) struct PixelCreateReq {
    provider: Provider,
    credentials: PixelCredentials,
    active: Option<bool>,
}

#[derive(Serialize)]
pub(crate) struct MaskedCredentials {
    measurement_id: Option<String>,
    api_secret: Option<String>,
    pixel_id: Option<String>,
    access_token: Option<String>,
}

/// Masks the secret fields (`api_secret`/`access_token`); `measurement_id`
/// and `pixel_id` are provider-facing identifiers, not secrets, so they pass
/// through unmasked.
pub(crate) fn mask_credentials(c: &PixelCredentials) -> MaskedCredentials {
    MaskedCredentials {
        measurement_id: c.measurement_id.clone(),
        api_secret: c.api_secret.as_ref().map(|_| MASKED_SECRET.to_string()),
        pixel_id: c.pixel_id.clone(),
        access_token: c.access_token.as_ref().map(|_| MASKED_SECRET.to_string()),
    }
}

#[derive(Serialize)]
pub(crate) struct PixelRow {
    id: u64,
    provider: Provider,
    credentials: MaskedCredentials,
    active: bool,
    created: u64,
}

pub(crate) fn to_pixel_row(config: &PixelConfig) -> PixelRow {
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
pub(crate) fn has_required_pixel_credentials(provider: Provider, c: &PixelCredentials) -> bool {
    fn non_empty(s: &Option<String>) -> bool {
        s.as_deref().map(|v| !v.trim().is_empty()).unwrap_or(false)
    }
    match provider {
        Provider::Ga4 => non_empty(&c.measurement_id) && non_empty(&c.api_secret),
        Provider::MetaCapi => non_empty(&c.pixel_id) && non_empty(&c.access_token),
    }
}

pub(crate) async fn admin_pixels_list(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
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
pub(crate) const MAX_API_TOKENS: usize = 100;

/// Token row shape for `GET /admin/tokens`: never includes the hash or the
/// plaintext, only what an operator needs to recognize/manage a token.
#[derive(Serialize)]
pub(crate) struct ApiTokenRow {
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

pub(crate) async fn admin_tokens_list(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
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

pub(crate) async fn admin_webhooks_create(
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
        label: None,
        connector_id: None,
        external_id: None,
        last_delivery_at: None,
        last_delivery_status: Default::default(),
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

pub(crate) async fn admin_webhooks_patch(
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

pub(crate) async fn wellknown_aasa(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    serve_wellknown(&st, "apple-app-site-association", &headers).await
}

pub(crate) async fn wellknown_assetlinks(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    serve_wellknown(&st, "assetlinks.json", &headers).await
}

pub(crate) async fn admin_wellknown_get(
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

pub(crate) async fn admin_wellknown_put(
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

pub(crate) async fn admin_webhooks_delete(
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
pub(crate) struct CreateTokenReq {
    name: String,
    scopes: Vec<Scope>,
    rate_limit_per_min: Option<u32>,
}

#[derive(Serialize)]
pub(crate) struct CreateTokenResp {
    id: u64,
    token: String,
}

pub(crate) async fn admin_tokens_create(
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

pub(crate) async fn admin_tokens_delete(
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

pub(crate) async fn admin_pixels_create(
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
        last_forward_at: None,
        last_forward_status: Default::default(),
    };
    if st.store.put_pixel(p.tenant, &config).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    (StatusCode::CREATED, Json(to_pixel_row(&config))).into_response()
}

pub(crate) async fn admin_pixels_delete(
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
pub(crate) async fn admin_webhooks_test(
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
pub(crate) async fn send_test_event_guarded(
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
        // This synthetic event never goes through the tenant-scoped worker
        // (`deliver_to_matching`/`lifecycle_deliveries`): it is built once
        // and sent straight to `sub` via `build_outgoing_request`, which
        // never reads `tenant_id`. The value here is inert; DEFAULT_TENANT
        // is used only to satisfy the type.
        tenant_id: crate::tenant::DEFAULT_TENANT,
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

pub(crate) async fn admin_wellknown_delete(
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
