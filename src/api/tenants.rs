use super::*;

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
pub(crate) fn log_keycloak_step_error(tenant_id: u64, step: &str, err: impl std::fmt::Display) {
    tracing::warn!(error = %err, step, tenant_id, "keycloak provisioning step failed");
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
/// `owner_user_id` is `Some` when the caller knows who the Owner is:
/// `admin_tenants_create` passes the creator, and the boot backfill looks it
/// up via `get_owner_user_id` (LUC-56). Their email drives `ensure_user`. It is
/// `None` only when a tenant has no Owner or the lookup failed; in that case the
/// admin-user step (and its set-password email) is skipped, but the
/// realm/client/groups/`oidc_config` still get provisioned. The same skip
/// happens when the owner's `User` row has no email on file.
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
            Ok(_) => tracing::warn!(
                tenant_id = tenant.id.0,
                "keycloak provisioning skipped, owner email unavailable"
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
        // Maps the quark-members group to Role::Member (write, no tenant
        // admin), so an invited Member has the same link write access as in
        // OSS single-tenant rather than collapsing to read-only Viewer.
        member_value: "quark-members".to_string(),
        // Default-closed: only quark-admins/quark-members/quark-readers members
        // are admitted (see `oidc::passes_required_group`), never the open
        // `Role::Member` fallback `claim_role` would otherwise grant to any
        // authenticated realm user.
        required_value: Some("quark-readers".to_string()),
        post_login_url: None,
        post_logout_url: None,
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
///
/// It also reconciles a stale `issuer` on tenants that already have a config
/// (LUC-81): when the Keycloak base URL changed after provisioning, the stored
/// issuer still points at the old base and SSO logins 401, so the issuer is
/// rewritten to the currently derived value (only that column; the encrypted
/// `client_secret` is never touched). Reconciles are logged and counted
/// separately, not folded into the returned provisioned count.
pub async fn backfill_keycloak_provisioning(
    store: &Arc<dyn Store>,
    keycloak: &Arc<dyn crate::keycloak::KeycloakAdmin>,
    base_url: &str,
) -> Result<usize, StoreError> {
    let tenants = store.list_tenants().await?;
    let mut provisioned = 0usize;
    let mut reconciled = 0usize;
    for t in &tenants {
        match store.get_oidc_config_bare(t.id).await {
            Ok(Some(cfg)) => {
                // Reconcile a stale issuer (LUC-81): the stored `issuer` is
                // derived from the Keycloak base URL at provision time, so if
                // `QUARK_KEYCLOAK_BASE_URL`/`KC_HOSTNAME` changed since, it
                // still points at the OLD base. Tokens now carry the new
                // issuer, but `verify` checks the token's `iss` against this
                // STORED value, so every existing tenant's SSO login 401s until
                // it's corrected. Rewrite ONLY the issuer to the currently
                // derived value (leaving the encrypted `client_secret` and the
                // rest of the blob untouched). Verification semantics are
                // unchanged: we still verify against the stored issuer, we're
                // just fixing the stored value. A correct issuer is left alone.
                let expected = crate::keycloak::derive_issuer(base_url, &t.slug);
                if cfg.issuer != expected {
                    match store.update_oidc_config_issuer(t.id, &expected).await {
                        Ok(()) => {
                            reconciled += 1;
                            tracing::info!(
                                issuer = %expected,
                                previous_issuer = %cfg.issuer,
                                tenant_id = t.id.0,
                                "keycloak issuer reconciled"
                            );
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, tenant_id = t.id.0, "keycloak backfill failed")
                        }
                    }
                }
                // Member-role backfill: tenants provisioned before the
                // `quark-members` group existed have an empty `member_value`
                // and no such group in their realm, so an invited Member
                // collapsed to read-only Viewer. Create the groups (idempotent)
                // and set `member_value` so `claim_role`/`passes_required_group`
                // recognise the member group. Scoped to Keycloak-provisioned
                // tenants (`client_id == "quark"`); an external-IdP tenant keeps
                // whatever it configured.
                if cfg.client_id == "quark" && cfg.member_value.is_empty() {
                    if let Err(e) = keycloak.ensure_groups_and_mapper(&t.slug).await {
                        tracing::warn!(error = %e, tenant_id = t.id.0, "keycloak backfill failed");
                    } else {
                        match store
                            .update_oidc_config_member_value(t.id, "quark-members")
                            .await
                        {
                            Ok(()) => {
                                reconciled += 1;
                                tracing::info!(
                                    member_value = "quark-members",
                                    tenant_id = t.id.0,
                                    "keycloak member value reconciled"
                                );
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, tenant_id = t.id.0, "keycloak backfill failed")
                            }
                        }
                    }
                }
            }
            Ok(None) => {
                // Provision the tenant's Owner in the realm too (LUC-56): a
                // tenant that predates Keycloak has an Owner membership but no
                // realm user, so without this its Owner could never SSO-log-in.
                // A lookup error/absent owner degrades to `None` (realm still
                // provisioned; owner step skipped), never failing the backfill.
                let owner = store.get_owner_user_id(t.id).await.unwrap_or(None);
                provision_tenant_keycloak(store, keycloak.as_ref(), base_url, t, owner).await;
                provisioned += 1;
            }
            Err(e) => {
                tracing::warn!(error = %e, tenant_id = t.id.0, "keycloak backfill failed")
            }
        }
    }
    // Reconciles are reported separately so the returned count keeps its
    // original meaning (tenants newly provisioned this pass, logged by
    // `main.rs`); a stale-issuer fix is not a fresh provision.
    if reconciled > 0 {
        tracing::info!(reconciled, "keycloak config backfill completed");
    }
    Ok(provisioned)
}

#[derive(Deserialize)]
pub(crate) struct CreateTenantReq {
    name: String,
    slug: String,
}

/// `POST /admin/tenants`: self-serve workspace creation (cloud only). Any
/// authenticated OIDC user may create a workspace — not gated by
/// `admin_guard`'s scope check, since a user with zero memberships must still
/// be able to create their first one. Creates the `Tenant`, grants the
/// caller `Owner` on it, and re-points their session at it.
pub(crate) async fn admin_tenants_create(
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
            Err(e) => {
                tracing::warn!(error = %e, tenant_id = id, "tenant subdomain seed failed")
            }
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

/// `DELETE /admin/tenants/:id`: workspace deletion (cloud only, LUC-138).
/// Irreversible: every row the tenant owns goes, the click events are queued
/// for deletion in the analytics backend, and the tenant's Keycloak realm is
/// removed.
///
/// Authorization, in this order, and every step matters:
/// 1. OSS answers `404` before any credential is looked at, like the other
///    workspace endpoints.
/// 2. A session is required, and an API token is deliberately NOT accepted:
///    destroying a workspace is not an automation operation. Same session-only
///    path `admin_tenants_create` takes.
/// 3. No membership in the target tenant is `404`, NOT `403`. A `403` would
///    confirm the tenant exists, which is customer enumeration; the answer has
///    to be indistinguishable from an id that never existed.
/// 4. Only `Owner`. `Admin` is refused even though `role_scopes` grants it the
///    same scopes (`src/tenant.rs`): the Admin role is derived from an IdP
///    claim group, so whoever controls the IdP could otherwise grant themselves
///    the workspace's destruction. `Owner` only ever comes from creating the
///    tenant or accepting an invite.
/// 5. The caller's last workspace is refused with `409`, so nobody deletes
///    themselves into having nowhere to land.
///
/// Order of effects (a spec decision, not an implementation detail): the store
/// transaction commits FIRST, and Keycloak/analytics follow best-effort. The
/// inverse would leave a live workspace whose realm is gone, which nobody can
/// log into. A failed `delete_realm` orphans a realm and logs; there is no
/// reaper, and that cost is accepted in the design doc.
///
/// No `csrf_guard` here, unlike `oidc_logout`. The session cookie is
/// `SameSite=None; Secure`, so a cross-site caller does carry it, but a `DELETE`
/// is never a CORS simple request: the browser always preflights it, and the
/// preflight is answered only for the configured origin allowlist (`router.rs`
/// builds the CORS layer without `Any` and with credentials allowed). A
/// cross-site page therefore never gets to send this request at all. The guard
/// exists for `oidc_logout` because a `POST` with a simple content type IS sent
/// without a preflight. Keep the allowlist strict: it is what protects this
/// endpoint.
pub(crate) async fn admin_tenants_delete(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<u64>,
) -> Response {
    if !st.multi_tenant {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(session) = current_session(&st, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let user_id = session.user_id;
    let target = crate::tenant::TenantId(id);
    match st.store.get_membership(user_id, target).await {
        // A tenant the caller has no membership in is reported exactly like a
        // tenant that does not exist. Do not "improve" this into a 403.
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Ok(Some(m)) if m.role != crate::tenant::Role::Owner => {
            return StatusCode::FORBIDDEN.into_response()
        }
        Ok(Some(_)) => {}
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    let memberships = match st.store.list_memberships_for_user(user_id).await {
        Ok(m) => m,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    if memberships.len() <= 1 {
        return (StatusCode::CONFLICT, "cannot delete your only workspace").into_response();
    }
    // Everything the delete needs to know about the tenant has to be read
    // BEFORE it: afterwards the rows are gone and there is no slug to name the
    // realm with, and no host list to evict from the router cache.
    let slug = match st.store.get_tenant(target).await {
        Ok(Some(t)) => Some(t.slug),
        Ok(None) => None,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    // Not best-effort: this read happens BEFORE the delete, and a swallowed
    // error would hand the loop at the bottom an empty host list, so not even
    // the node serving this request would evict its own router cache and the
    // deleted workspace's links would keep redirecting until the TTL expires.
    // Failing here costs nothing, the workspace is still intact.
    let hosts: Vec<String> = match st.store.list_domains(target).await {
        Ok(domains) => domains.into_iter().map(|d| d.host).collect(),
        Err(e) => {
            tracing::warn!(error = %e, tenant_id = id, "workspace delete aborted: domain list failed");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    };

    if let Err(e) = st.store.delete_tenant(target).await {
        // The whole delete is one transaction, so nothing was removed and the
        // caller can retry against an intact workspace.
        tracing::warn!(error = %e, tenant_id = id, "workspace delete failed");
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    tracing::info!(tenant_id = id, user_id, "workspace deleted");

    // Best-effort from here down: the workspace is already gone, so none of
    // these may turn a successful deletion into an error response.
    if let Err(e) = st.sink.delete_tenant_data(id).await {
        tracing::warn!(error = %e, tenant_id = id, "tenant analytics delete failed");
    }
    if let (Some(kc), Some(slug)) = (&st.keycloak, &slug) {
        if let Err(e) = kc.delete_realm(slug).await {
            tracing::warn!(error = %e, slug = %slug, tenant_id = id, "realm delete failed");
        }
    }

    // Deleting the workspace the caller is sitting in must not log them out.
    // `sessions` is tenant-owned, so the row just went down with the tenant;
    // re-issue it against a workspace they still belong to (step 5 guarantees
    // one exists). A delete of some OTHER workspace leaves the session alone.
    if session.tenant_id == target {
        if let Some(next) = memberships.iter().find(|m| m.tenant_id != target) {
            let mut session = session;
            session.tenant_id = next.tenant_id;
            if let Err(e) = st.store.put_session(next.tenant_id, &session).await {
                tracing::warn!(error = %e, tenant_id = id, "session reissue after delete failed");
            }
        }
    }

    for host in hosts {
        st.host_router.invalidate(&host).await;
    }
    st.oidc_tenants.invalidate(target).await;
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
pub(crate) struct SwitchReq {
    tenant_id: u64,
}

/// `POST /admin/workspace/switch`: change the session's current workspace
/// (cloud only). SECURITY: always validates membership before switching — a
/// caller may only switch to a tenant they belong to. A missing membership
/// leaves the session untouched and returns `403`, rather than mutating it
/// and failing closed some other way.
pub(crate) async fn admin_workspace_switch(
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
