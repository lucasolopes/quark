use super::*;

/// How long a team invite stays valid before it must be re-sent.
pub(crate) const INVITE_TTL_SECS: u64 = 7 * 24 * 3600;

#[derive(Deserialize)]
pub(crate) struct CreateInviteReq {
    email: String,
    role: crate::tenant::Role,
}

#[derive(Serialize)]
pub(crate) struct CreateInviteResp {
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
pub(crate) async fn admin_invites_create(
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
pub(crate) struct InviteView {
    id: u64,
    email: String,
    role: crate::tenant::Role,
    expires: u64,
    created: u64,
}

/// `GET /admin/invites`: pending invites for the caller's tenant (cloud only,
/// Owner/Admin required). Never includes `token_hash`.
pub(crate) async fn admin_invites_list(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
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
pub(crate) async fn admin_invites_delete(
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
    // 204 to match the other admin deletes (LUC-51): a successful delete has no body.
    StatusCode::NO_CONTENT.into_response()
}

// --- Per-tenant OIDC config CRUD (multi-tenancy P2d Task 2), cloud-only ---

#[derive(Deserialize)]
pub(crate) struct PutOidcConfigReq {
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
pub(crate) struct OidcConfigView {
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
pub(crate) async fn admin_oidc_config_put(
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
        // Not part of the admin OIDC-config API surface (RP-initiated logout
        // uses the global runtime's end-session endpoint, LUC-79).
        post_logout_url: None,
    };
    if let Err(e) = st.store.put_oidc_config(&cfg).await {
        return conflict_or_503(e).into_response();
    }
    st.oidc_tenants.invalidate(p.tenant).await;
    Json(OidcConfigView::from(&cfg)).into_response()
}

/// `GET /admin/oidc-config`: the caller's tenant's own OIDC IdP, redacted
/// (cloud only, Owner/Admin required). `404` when the tenant has none set up.
pub(crate) async fn admin_oidc_config_get(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
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
pub(crate) async fn admin_oidc_config_delete(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
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
pub(crate) struct AcceptInviteResp {
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
/// hides all three), email match, existing-membership conflict, then
/// `accept_invite_tx` claims the invite row AND grants the membership in one
/// backend transaction (LUC-46): two concurrent accepts of the same token
/// both pass the checks above, but only one wins the claim, and if the grant
/// half fails the claim is rolled back too, so a write failure never
/// strands the invite as consumed-but-unmembered.
pub(crate) async fn admin_invites_accept(
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
    let membership = crate::tenant::Membership {
        user_id,
        tenant_id: inv.tenant_id,
        role: inv.role,
        created: now(),
    };
    match st.store.accept_invite_tx(inv.id, &membership, now()).await {
        Ok(true) => {}
        // Lost the race, or the invite was already consumed between the
        // lookup above and here: treat it the same as an unknown token.
        Ok(false) => return StatusCode::NOT_FOUND.into_response(),
        // Atomic: an error here rolled the whole transaction back, so the
        // claim was NOT taken and the invite is still pending.
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    set_session_tenant(&st, &headers, inv.tenant_id).await;
    Json(AcceptInviteResp {
        tenant_id: inv.tenant_id.0,
        role: inv.role,
    })
    .into_response()
}
