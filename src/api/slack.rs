use super::*;

/// Name of the short-lived cookie holding the signed Slack OAuth `state`,
/// binding the install flow to the browser that started it (anti login-CSRF).
/// Mirrors the Sheets connect flow.
pub(crate) const SLACK_STATE_COOKIE: &str = "qk_slack_state";

/// `GET /admin/integrations/slack/connect`: begin the Slack "Add to Slack"
/// install. Called by the panel via `fetch` with its admin credential, so it
/// returns the Slack authorize URL as JSON (not a 303) and sets a signed,
/// short-lived `state` cookie; the panel then navigates the browser there.
/// The signed cookie's `verifier` slot carries the calling tenant across the
/// top-level redirect and back, so the callback persists under the same tenant.
pub(crate) async fn slack_connect(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let p = match admin_guard(&st, &headers, Scope::Webhooks).await {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    let Some(cfg) = st.slack.as_ref() else {
        return sheets_off_status(&st).into_response();
    };
    let state = crate::oidc::random_token();
    let signed =
        crate::oidc::sign_login_state(&st.signing_key, &state, &p.tenant.0.to_string(), "", None);
    let url = crate::slack::connect_url(cfg, &state);
    let secure = if request_is_https(&headers) { "; Secure" } else { "" };
    let cookie =
        format!("{SLACK_STATE_COOKIE}={signed}; Max-Age=600; Path=/; HttpOnly; SameSite=Lax{secure}");
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
pub(crate) struct SlackCallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// The events a fresh Slack channel subscription is registered for: every link
/// lifecycle event, matching a comprehensive notification channel. Health-only
/// events (`link.broken`/`link.recovered`) simply never fire when broken-link
/// monitoring is off, so including them is harmless.
fn all_events() -> Vec<EventType> {
    vec![
        EventType::LinkCreated,
        EventType::LinkUpdated,
        EventType::LinkDeleted,
        EventType::LinkExpired,
        EventType::LinkClicked,
        EventType::LinkBroken,
        EventType::LinkRecovered,
        EventType::LinkThresholdReached,
    ]
}

/// `GET /admin/integrations/slack/callback`: verify the state cookie matches the
/// echoed `state`, exchange the code for the incoming-webhook URL, persist it as
/// a `kind: Slack` webhook subscription under the tenant that started the flow,
/// clear the state cookie, and redirect back to the panel's Slack view.
pub(crate) async fn slack_callback(
    State(st): State<Arc<AppState>>,
    Query(params): Query<SlackCallbackParams>,
    headers: HeaderMap,
) -> Response {
    // No `admin_guard`: this is a top-level browser redirect from Slack with no
    // admin credential. Authorized by the same double-submit `state` check the
    // Sheets connect uses (signed cookie set at /connect must match the echoed
    // value), which binds the flow to this browser.
    let Some(cfg) = st.slack.as_ref() else {
        return sheets_off_status(&st).into_response();
    };
    if params.error.is_some() {
        return (StatusCode::UNAUTHORIZED, "install was denied at Slack").into_response();
    }
    let verified = cookie_value(&headers, SLACK_STATE_COOKIE)
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
        return (StatusCode::BAD_REQUEST, "missing or invalid install state").into_response();
    }
    let Some(code) = params.code else {
        return (StatusCode::BAD_REQUEST, "missing code").into_response();
    };
    let access = match crate::slack::exchange_code(&reqwest_client(), cfg, &code).await {
        Ok(a) => a,
        Err(_) => return (StatusCode::BAD_GATEWAY, "slack oauth exchange failed").into_response(),
    };
    if !access.ok {
        return (StatusCode::BAD_GATEWAY, "slack rejected the install").into_response();
    }
    let Some(webhook) = access.incoming_webhook else {
        return (StatusCode::BAD_GATEWAY, "no incoming webhook in slack response").into_response();
    };
    let url = webhook.url;
    // The channel the operator picked (e.g. "#general"), shown in the panel so
    // multiple Slack connections can be told apart.
    let label = webhook.channel.filter(|c| !c.is_empty());
    // The URL comes from Slack (`hooks.slack.com`), but still run it through the
    // same guard every stored webhook destination passes (defense in depth).
    if validate_webhook_url(&url).is_err() {
        return (StatusCode::BAD_GATEWAY, "slack returned an unusable webhook url").into_response();
    }
    let existing = match st.store.list_webhooks(tenant).await {
        Ok(subs) => subs,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    // Idempotent: re-installing the SAME channel (a double-click, or a repeat
    // "Add to Slack" for a channel already connected) must not create a
    // duplicate subscription. Slack returns the same incoming webhook URL for
    // the same channel, so match on it and just return to the panel.
    if let Some(dup) = existing
        .iter()
        .find(|s| s.kind == SubscriptionKind::Slack && s.url == url)
    {
        // Backfill the channel label if this connection predates label capture
        // (or was made manually) and Slack now told us the channel.
        if dup.label.is_none() && label.is_some() {
            let updated = WebhookSubscription {
                label,
                ..dup.clone()
            };
            let _ = st.store.put_webhook(tenant, &updated).await;
        }
        let clear = format!("{SLACK_STATE_COOKIE}=; Max-Age=0; Path=/; HttpOnly; SameSite=Lax");
        return (
            StatusCode::SEE_OTHER,
            [
                (header::LOCATION, slack_return_url(&st)),
                (header::SET_COOKIE, clear),
                (header::CACHE_CONTROL, "no-store".to_string()),
            ],
        )
            .into_response();
    }
    // Enforce the same per-tenant subscription cap the manual create does.
    if existing.len() >= MAX_WEBHOOK_SUBSCRIPTIONS {
        return (StatusCode::BAD_REQUEST, "webhook subscription cap reached").into_response();
    }
    let id = match st.store.next_webhook_id(tenant).await {
        Ok(id) => id,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let sub = WebhookSubscription {
        id,
        url,
        events: all_events(),
        secret: String::new(),
        active: true,
        created: now(),
        kind: SubscriptionKind::Slack,
        label,
    };
    if st.store.put_webhook(tenant, &sub).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let clear = format!("{SLACK_STATE_COOKIE}=; Max-Age=0; Path=/; HttpOnly; SameSite=Lax");
    (
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, slack_return_url(&st)),
            (header::SET_COOKIE, clear),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
    )
        .into_response()
}

/// Where to send the browser after a Slack install: the panel's dedicated Slack
/// view. On a split-domain deploy the backend root is POST-only, so use the
/// global OIDC post-login URL (the panel base); falls back to "/" for OSS.
fn slack_return_url(st: &AppState) -> String {
    st.oidc
        .as_ref()
        .map(|rt| rt.config.post_login_url.trim_end_matches('/').to_string())
        .filter(|u| !u.is_empty() && u != "/")
        .map(|u| format!("{u}/extensions/slack"))
        .unwrap_or_else(|| "/".to_string())
}
