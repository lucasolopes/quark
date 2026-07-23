use super::*;

/// Name of the short-lived cookie holding the signed Sheets OAuth `state`,
/// binding the connect flow to the browser that started it (anti login-CSRF).
pub(crate) const SHEETS_STATE_COOKIE: &str = "qk_sheets_state";

/// `GET /admin/integrations/sheets/connect`: begin the Google OAuth connect.
/// Called by the panel via `fetch` with its admin credential, so it returns the
/// Google consent URL as JSON (rather than a 303) and sets a signed, short-lived
/// `state` cookie; the panel then navigates the browser to that URL. Returning
/// JSON lets a token-authenticated operator start the flow (a top-level redirect
/// could not carry the `x-admin-token` header). Returns the admin-surface
/// not-found status when the connector is not configured.
pub(crate) async fn sheets_connect(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
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
pub(crate) struct SheetsCallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// Decodes the `email` claim from a Google id_token WITHOUT verifying the
/// signature: the token came straight from Google's token endpoint over TLS, so
/// it is trusted here. Returns `""` on a missing or malformed token.
pub(crate) fn email_from_id_token(id_token: Option<&str>) -> String {
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
pub(crate) async fn sheets_callback(
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
            (header::LOCATION, sheets_return_url(&st)),
            (header::SET_COOKIE, clear),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
    )
        .into_response()
}

/// Where to send the browser after a Sheets connect. On a split-domain deploy
/// the panel is a different origin and the backend root is POST-only (a bare
/// "/" would 405), so return to the panel's Extensions page via the global OIDC
/// post-login URL (the panel base). Falls back to "/" for OSS single-origin.
fn sheets_return_url(st: &AppState) -> String {
    st.oidc
        .as_ref()
        .map(|rt| rt.config.post_login_url.trim_end_matches('/').to_string())
        .filter(|u| !u.is_empty() && u != "/")
        .map(|u| format!("{u}/extensions"))
        .unwrap_or_else(|| "/".to_string())
}

/// The Sheets status the panel renders. Deliberately its own struct (never
/// `SheetsConnection`) so the `refresh_token` can NEVER be serialized here.
#[derive(Serialize)]
pub(crate) struct SheetsStatusResponse {
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
pub(crate) async fn sheets_status(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
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
pub(crate) async fn sheets_sync(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
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
                    p.tenant,
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
pub(crate) async fn sheets_disconnect(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
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
pub(crate) fn sheets_off_status(st: &AppState) -> StatusCode {
    if st.admin_token.is_some() || st.oidc_configured {
        StatusCode::UNAUTHORIZED
    } else {
        StatusCode::NOT_FOUND
    }
}

/// The public host quark serves on, for building `short_url`s in the synced
/// sheet: the configured `public_host`, else the request `Host` header, else a
/// placeholder. Mirrors how `is_blocked_target` derives the self host.
pub(crate) fn sheets_base_host(st: &AppState, headers: &HeaderMap) -> String {
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
pub(crate) fn reqwest_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(HTTP_CLIENT_TIMEOUT_SECS))
        .build()
        .expect("reqwest client builds")
}
