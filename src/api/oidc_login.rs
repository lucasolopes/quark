use super::*;

/// Name of the session cookie set after a successful OIDC login.
pub(crate) const SESSION_COOKIE: &str = "qk_session";
/// Name of the short-lived cookie carrying the login-attempt state (PKCE
/// verifier + state + nonce) from `/admin/login` to `/admin/callback`.
pub(crate) const LOGIN_COOKIE: &str = "qk_login";
/// How long a login session lasts.
pub(crate) const SESSION_TTL_SECS: u64 = 12 * 3600;

#[derive(Deserialize)]
pub(crate) struct LoginParams {
    /// Tenant slug for a per-tenant login (multi-tenancy P2d,
    /// `/admin/login?org=<slug>`). Absent (or empty) means the global/OSS
    /// login against the env-configured IdP, unchanged.
    pub(crate) org: Option<String>,
    /// Optional email to forward to the IdP as `login_hint` so the provider
    /// pre-fills the username. Purely a UX convenience; the server never trusts
    /// it for authorization.
    pub(crate) login_hint: Option<String>,
}

/// The single, generic 404 for every `?org=<slug>` login failure that must
/// not leak whether the slug is real (multi-tenancy P2d Task 4b): an unknown
/// slug and a real tenant with no OIDC config of its own are otherwise
/// distinguishable messages, which would let an unauthenticated caller
/// enumerate valid organization slugs one probe at a time.
pub(crate) fn org_login_not_found() -> Response {
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
pub(crate) async fn oidc_login(
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
            // Resolution against the tenant's own IdP is Enterprise (LUC-145);
            // the Community build resolves nothing and lands on the same 404.
            match tenant_idp::runtime_for_slug(&st, slug, &headers).await {
                Ok((rt, tenant_id)) => (rt, Some(tenant_id)),
                Err(tenant_idp::TenantIdpError::RateLimited) => {
                    return (StatusCode::TOO_MANY_REQUESTS, "too many requests").into_response()
                }
                Err(tenant_idp::TenantIdpError::Unavailable) => {
                    return StatusCode::SERVICE_UNAVAILABLE.into_response()
                }
                Err(tenant_idp::TenantIdpError::Unreachable) => {
                    return (
                        StatusCode::BAD_GATEWAY,
                        "organization's identity provider is unreachable",
                    )
                        .into_response()
                }
                Err(tenant_idp::TenantIdpError::NotFound) => return org_login_not_found(),
            }
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
    let url = runtime.authorize_url(&state, &nonce, &challenge, params.login_hint.as_deref());
    let value = crate::oidc::sign_login_state(
        st.signing_key.expose_secret(),
        &state,
        &verifier,
        Some(&nonce),
        tenant,
    );
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

/// Why `member_quota_allows_login` refused a login. Distinct from
/// `entitlement::Denied` alone because a store failure here must become a
/// `503`, not a `402` misreported as a plan limit.
///
/// `pub`, not `pub(crate)`, so the Enterprise integration suite
/// (`tests/plan_it.rs`) can call `member_quota_allows_login` directly: the
/// real callback can't be driven end to end offline (it needs a live IdP for
/// the code exchange), so this is the finest-grained seam that is both
/// testable without one and actually wired into `oidc_callback`.
#[derive(Debug)]
pub enum MemberLoginDenied {
    /// The plan's member ceiling is already held and `subject` would be a
    /// new member.
    Quota(crate::api::entitlement::Denied),
    /// Could not determine whether `subject` is already a member (store
    /// unreachable), so the login cannot be safely evaluated either way.
    StoreUnavailable,
}

impl IntoResponse for MemberLoginDenied {
    fn into_response(self) -> Response {
        match self {
            MemberLoginDenied::StoreUnavailable => StatusCode::SERVICE_UNAVAILABLE.into_response(),
            // Same shape as `entitlement::Denied`'s own `IntoResponse`, but
            // with a login-specific error code: the panel needs to tell "your
            // login was refused, the workspace is full" apart from a normal
            // API call hitting a plan limit, even though both are `402`s
            // carrying the same `limit`/`allowed`/`upgrade_to` triple.
            MemberLoginDenied::Quota(denied) => (
                StatusCode::PAYMENT_REQUIRED,
                Json(serde_json::json!({
                    "error": "member_limit_reached",
                    "limit": denied.limit,
                    "allowed": denied.allowed,
                    "upgrade_to": denied.upgrade_to,
                })),
            )
                .into_response(),
        }
    }
}

/// Whether the plan's member quota admits `subject` logging into `tenant` via
/// its own IdP group-claim mapping (multi-tenancy model B / Keycloak,
/// LUC-148).
///
/// Gates ONLY a brand-new member: a `subject` that already holds a
/// `Membership` in `tenant` is exempt, so a plan downgrade never locks out
/// anyone already in — the same "existing grants are never revoked" rule
/// phase 1 applied to domains and invites (`docs/specs/...luc19...` and the
/// invite quota check in `src/ee/api/invites.rs`). A `subject` with no `User`
/// row at all is a new member by definition (it cannot hold a membership
/// either), so it is evaluated the same way.
///
/// Community's `require_quota` always answers `Ok`, so this only bites
/// Enterprise builds whose tenant plan caps members below what it already
/// holds.
pub async fn member_quota_allows_login(
    st: &AppState,
    tenant: crate::tenant::TenantId,
    subject: &str,
) -> Result<(), MemberLoginDenied> {
    let existing_user = st
        .store
        .get_user_by_subject(subject)
        .await
        .map_err(|_| MemberLoginDenied::StoreUnavailable)?;
    if let Some(user) = existing_user {
        let membership = st
            .store
            .get_membership(user.id, tenant)
            .await
            .map_err(|_| MemberLoginDenied::StoreUnavailable)?;
        if membership.is_some() {
            // Already a member of this tenant: never re-gated by the ceiling
            // on a later login, no matter what the plan does afterwards.
            return Ok(());
        }
    }
    let held = st
        .store
        .count_memberships(tenant)
        .await
        .map_err(|_| MemberLoginDenied::StoreUnavailable)?;
    crate::api::entitlement::require_quota(
        st,
        tenant,
        crate::api::entitlement::Quota::Members,
        held,
    )
    .await
    .map_err(MemberLoginDenied::Quota)
}

#[derive(Deserialize)]
pub(crate) struct CallbackParams {
    pub(crate) code: Option<String>,
    pub(crate) state: Option<String>,
    pub(crate) error: Option<String>,
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
pub(crate) async fn oidc_callback(
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
        .and_then(|c| crate::oidc::verify_login_state(st.signing_key.expose_secret(), c));
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
    // CSRF: the state echoed by the IdP must match the one we signed. Compared
    // in constant time, like the Sheets and Slack callbacks do: `state` is a
    // secret the attacker must not be able to recover a prefix of by timing.
    let state_matches = match params.state.as_deref() {
        Some(echoed) => constant_time_eq(echoed.as_bytes(), state.as_bytes()),
        None => false,
    };
    if !state_matches {
        return (StatusCode::BAD_REQUEST, "state mismatch").into_response();
    }
    let Some(code) = params.code else {
        return (StatusCode::BAD_REQUEST, "missing code").into_response();
    };

    // Resolve which IdP runtime and config validate this callback: the
    // tenant signed into the login cookie (multi-tenancy P2d), or the global
    // env-configured IdP.
    let (runtime, tenant_cfg) = match tenant {
        Some(tenant_id) => match tenant_idp::runtime_for_tenant(&st, tenant_id).await {
            Ok((rt, cfg)) => (rt, Some(cfg)),
            // The tenant's config was removed after the login started, or this
            // is a Community build that cannot resolve one at all. Either way
            // the login cannot be completed against a per-tenant IdP.
            Err(tenant_idp::TenantIdpError::NotFound) => {
                return (
                    StatusCode::BAD_REQUEST,
                    "organization's identity provider is no longer configured",
                )
                    .into_response()
            }
            Err(tenant_idp::TenantIdpError::Unreachable) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    "organization's identity provider is unreachable",
                )
                    .into_response()
            }
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        },
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
                // Fall back to the GLOBAL post-login URL (the panel) before "/":
                // in a split-domain deploy the panel lives on a different host
                // than the API, so "/" would land on the API root (a 405). A
                // per-tenant login should return to the same panel as the
                // global one when the tenant config doesn't override it.
                cfg.post_login_url
                    .clone()
                    .or_else(|| st.oidc.as_ref().map(|rt| rt.config.post_login_url.clone()))
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

    // Member-quota gate (LUC-148): only a per-tenant login that would create
    // a BRAND NEW membership can be refused here, checked before
    // `ensure_user_and_membership` grants anything (that call is
    // unconditional for whoever reaches it). Someone who already has a
    // membership in `tenant` sails through regardless of the plan's current
    // ceiling; a plan downgrade never revokes access from an existing
    // member, only blocks a new one from joining.
    if st.multi_tenant {
        if let Some((tenant_id, _)) = tenant_membership {
            if let Err(denied) = member_quota_allows_login(&st, tenant_id, &claims.subject).await {
                return denied.into_response();
            }
        }
    }

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
        // Kept so RP-initiated logout can pass it as `id_token_hint` to end
        // the IdP session too (not just quark's local session).
        id_token: Some(id_token.clone()),
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

/// `POST /admin/logout`: revoke the current session, clear the cookie, and
/// hand the panel the IdP's RP-initiated logout URL (OIDC end-session) so the
/// browser can also end the IdP session, not just quark's local one (LUC-79).
/// Requires the `x-quark-csrf` header the panel sends: without it a cross-site
/// simple POST could force-logout via the SameSite=None cookie, and with it the
/// request is preflighted so the CORS allowlist gates any cross-origin caller.
///
/// Responds `200 OK` with `{"logout_url": <string|null>}`: the URL is `null`
/// (local-only logout, as before) when OIDC is off, the IdP advertised no
/// `end_session_endpoint`, or the session carried no stored `id_token`.
pub(crate) async fn oidc_logout(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(status) = csrf_guard(&headers) {
        return status.into_response();
    }
    // Read the session BEFORE deleting it, so we still have its `id_token` (the
    // end-session hint) and `tenant_id` (which realm issued it).
    let mut id_token: Option<String> = None;
    let mut sess_tenant = crate::tenant::DEFAULT_TENANT;
    if let Some(raw) = cookie_value(&headers, SESSION_COOKIE) {
        let hash = hash_token(raw);
        if let Ok(Some(session)) = st.store.get_session_by_hash(&hash, now()).await {
            id_token = session.id_token;
            sess_tenant = session.tenant_id;
        }
        let _ = st.store.delete_session(&hash).await;
    }
    // Build the RP-initiated logout URL on the realm the session belongs to. A
    // per-tenant session (multi-tenancy P2d) was issued by the TENANT's realm,
    // so its `id_token` must go to that realm's end-session endpoint, not the
    // global one (a cross-realm `id_token_hint` is rejected with 400). Global
    // sessions (DEFAULT tenant) use the env-configured IdP. `None` (local-only
    // logout) whenever a piece is missing.
    // Where to send the browser after the IdP ends the session: the GLOBAL
    // config's post-logout URL (the panel), falling back to its post-login URL
    // then "/". The panel is the same for every realm, so a per-tenant logout
    // returns here too (the tenant client allows it via `post.logout.redirect
    // .uris`), instead of the tenant config's own (usually `/`) value.
    let redirect = st
        .oidc
        .as_ref()
        .map(|rt| {
            rt.config
                .post_logout_url
                .clone()
                .unwrap_or_else(|| rt.config.post_login_url.clone())
        })
        .unwrap_or_else(|| "/".to_string());
    let logout_url = match id_token.as_deref() {
        None => None,
        Some(tok) => {
            // Route the logout to the realm that ISSUED this token (its `iss`),
            // not the session's tenant: a global sign-in can leave the session
            // pointed at a tenant, so `tenant_id` may name a different realm
            // than the one that minted the id_token, and a cross-realm
            // `id_token_hint` is rejected with 400.
            let iss = crate::oidc::token_issuer(tok);
            let is_global = st
                .oidc
                .as_ref()
                .is_some_and(|rt| Some(rt.config.issuer.as_str()) == iss.as_deref());
            if is_global {
                st.oidc
                    .as_ref()
                    .and_then(|rt| rt.logout_url(tok, &redirect))
            } else if sess_tenant != crate::tenant::DEFAULT_TENANT {
                // Per-tenant sign-in: the tenant's own realm issued the token.
                match tenant_idp::runtime_for_tenant(&st, sess_tenant).await {
                    Ok((rt, _)) => rt.logout_url(tok, &redirect),
                    Err(_) => None,
                }
            } else {
                None
            }
        }
    };
    let clear = format!("{SESSION_COOKIE}=; Max-Age=0; Path=/; HttpOnly; SameSite=Lax");
    (
        StatusCode::OK,
        [
            (header::SET_COOKIE, clear),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        Json(serde_json::json!({ "logout_url": logout_url })),
    )
        .into_response()
}

/// `GET /admin/me`: the current principal (from the session cookie) plus whether
/// OIDC is configured, so the panel can render the login button and signed-in
/// state. Never guarded: it reports `authenticated: false` instead of erroring.
pub(crate) async fn admin_me(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let oidc_enabled = st.oidc.is_some();
    // The optional custom label for the shared OIDC login button (from
    // `QUARK_OIDC_BUTTON_LABEL`), read off the live runtime's config. Absent
    // when unset so the panel falls back to its i18n label.
    let oidc_button_label = st
        .oidc
        .as_ref()
        .and_then(|rt| rt.config.button_label.clone());
    if let Some(raw) = cookie_value(&headers, SESSION_COOKIE) {
        if let Ok(Some(session)) = st.store.get_session_by_hash(&hash_token(raw), now()).await {
            let mut body = serde_json::json!({
                "authenticated": true,
                "display": session.display,
                "scopes": session.scopes,
                "oidc_enabled": oidc_enabled,
                "oidc_button_label": oidc_button_label,
                "multi_tenant": st.multi_tenant,
                "admin_login_enabled": st.admin_token.is_some(),
                "sso_provisioning": sso_provisioning(&st),
                "tenant_domain_suffix": st.tenant_domain_suffix,
                "public_host": st.public_host,
                "slack_connect": st.slack.is_some(),
                "edition": st.license.edition(),
            });
            // Cloud (multi-tenant) only: the panel gates workspace onboarding on
            // the PRESENCE of `memberships`, so OSS/single-tenant MUST omit both
            // fields entirely (an empty `[]` would read as cloud and trap the
            // user on the workspace gate instead of the app). See
            // `web/src/app/RequireAuth.tsx`.
            if st.multi_tenant {
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
                // The current workspace is the session's tenant ONLY when the user
                // actually has a membership there. A fresh cloud user's session
                // still carries DEFAULT_TENANT (0) with no membership in it, so
                // report `null` to signal onboarding rather than a phantom
                // "workspace 0".
                let current_tenant = match st
                    .store
                    .get_membership(session.user_id, session.tenant_id)
                    .await
                {
                    Ok(Some(_)) => Some(session.tenant_id.0),
                    _ => None,
                };
                body["memberships"] = serde_json::json!(out);
                body["current_tenant"] = serde_json::json!(current_tenant);
                // Host the panel uses to build/copy short links (LUC-86): the
                // tenant's primary custom domain, else its subdomain, else the
                // shared host. Only meaningful once a workspace is selected.
                if current_tenant.is_some() {
                    body["primary_link_host"] =
                        serde_json::json!(primary_link_host(&st, session.tenant_id).await);
                }
            }
            return Json(body).into_response();
        }
    }
    Json(serde_json::json!({
        "authenticated": false,
        "oidc_enabled": oidc_enabled,
        "oidc_button_label": oidc_button_label,
        "multi_tenant": st.multi_tenant,
        "admin_login_enabled": st.admin_token.is_some(),
        "sso_provisioning": sso_provisioning(&st),
        "tenant_domain_suffix": st.tenant_domain_suffix,
        "public_host": st.public_host,
        "slack_connect": st.slack.is_some(),
        "edition": st.license.edition(),
    }))
    .into_response()
}
