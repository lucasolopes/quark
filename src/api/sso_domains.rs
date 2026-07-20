use super::*;

/// Name of the TXT record a caller must publish to prove ownership of an
/// email domain for SSO discovery (mirrors `verify_txt_name`, but under its
/// own label so it never collides with the P3 custom-domain record).
pub(crate) fn sso_verify_txt_name(domain: &str) -> String {
    format!("_quark-sso.{domain}")
}

/// An SSO email domain plus the DNS instructions to verify it (mirrors
/// `DomainView`).
#[derive(Serialize)]
pub(crate) struct SsoDomainView {
    id: u64,
    domain: String,
    status: DomainStatus,
    created: u64,
    verified_at: Option<u64>,
    /// Name of the TXT record to publish: `_quark-sso.<domain>`.
    txt_name: String,
    /// Value the TXT record must hold (the domain's verification token).
    txt_value: String,
}

pub(crate) fn sso_domain_view(d: &SsoEmailDomain) -> SsoDomainView {
    SsoDomainView {
        id: d.id,
        domain: d.domain.clone(),
        status: d.status,
        created: d.created,
        verified_at: d.verified_at,
        txt_name: sso_verify_txt_name(&d.domain),
        txt_value: d.token.clone(),
    }
}

#[derive(Deserialize)]
pub(crate) struct CreateSsoDomainReq {
    domain: String,
}

/// `GET /admin/sso-domains`: list the caller's tenant's SSO email domains,
/// each with the DNS instructions needed to verify it (cloud only).
pub(crate) async fn admin_sso_domains_list(
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
    match st.store.list_sso_domains(p.tenant).await {
        Ok(domains) => {
            Json(domains.iter().map(sso_domain_view).collect::<Vec<_>>()).into_response()
        }
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// `POST /admin/sso-domains {domain}`: registers an email domain for SSO
/// discovery under the caller's tenant, pending DNS verification (cloud
/// only). Requires the tenant to already have an `oidc_config`: there is no
/// point discovering a domain into a tenant with nowhere to route the login
/// -- that gate returns `409` ("SSO not configured"). Normalizes `domain`
/// (lowercase, trimmed) and rejects anything that isn't a plausible domain
/// (reuses `normalize_email_domain` by prepending a dummy local part).
/// Duplicate `domain` (unique across tenants) -> `409`.
pub(crate) async fn admin_sso_domains_create(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateSsoDomainReq>,
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
    match st.store.get_oidc_config_bare(p.tenant).await {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::CONFLICT, "SSO not configured").into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    let raw = req.domain.trim().to_ascii_lowercase();
    let Some(domain) = normalize_email_domain(&format!("x@{raw}")) else {
        return (StatusCode::BAD_REQUEST, "invalid domain").into_response();
    };
    let id = match st.store.next_sso_domain_id().await {
        Ok(i) => i,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let sso_domain = SsoEmailDomain {
        id,
        tenant_id: p.tenant,
        domain,
        token: generate_token(),
        status: DomainStatus::Pending,
        created: now(),
        verified_at: None,
    };
    if let Err(e) = st.store.put_sso_domain(&sso_domain).await {
        return conflict_or_503(e).into_response();
    }
    Json(sso_domain_view(&sso_domain)).into_response()
}

/// `POST /admin/sso-domains/:id/verify`: looks up the `_quark-sso.<domain>`
/// TXT record for the caller's tenant's SSO email domain; on a match, marks
/// it `Verified`. A missing or mismatched TXT record leaves it `Pending`
/// (mirrors `admin_domains_verify`).
pub(crate) async fn admin_sso_domains_verify(
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
    let mut sso_domain = match st.store.get_sso_domain(p.tenant, id).await {
        Ok(Some(d)) => d,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let txt_name = sso_verify_txt_name(&sso_domain.domain);
    let matched = st
        .dns
        .lookup_txt(&txt_name)
        .await
        .map(|values| values.iter().any(|v| v == &sso_domain.token))
        .unwrap_or(false);
    if matched {
        let verified_at = now();
        if st
            .store
            .set_sso_domain_status(p.tenant, id, DomainStatus::Verified, Some(verified_at))
            .await
            .is_err()
        {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        sso_domain.status = DomainStatus::Verified;
        sso_domain.verified_at = Some(verified_at);
    }
    Json(sso_domain_view(&sso_domain)).into_response()
}

/// `DELETE /admin/sso-domains/:id`: removes the caller's tenant's SSO email
/// domain (cloud only).
pub(crate) async fn admin_sso_domains_delete(
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
    match st.store.get_sso_domain(p.tenant, id).await {
        Ok(Some(_)) => {}
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    if st.store.delete_sso_domain(p.tenant, id).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
pub(crate) struct DiscoverParams {
    email: Option<String>,
}

/// Response for `GET /admin/sso/discover`. Deliberately carries only the
/// tenant's slug (already public via `/admin/login?org=<slug>`) and never
/// `tenant_id`, and is shaped identically whether the email is malformed, the
/// domain is unknown, still pending, or its tenant lost its `oidc_config` --
/// an unauthenticated caller must not be able to tell those cases apart
/// (anti-enumeration; mirrors `org_login_not_found`'s uniform-404 intent, but
/// this endpoint is a lookup so it stays `200` throughout).
#[derive(Serialize, Default)]
pub(crate) struct DiscoverResp {
    #[serde(skip_serializing_if = "Option::is_none")]
    org: Option<String>,
}

/// `GET /admin/sso/discover?email=<email>`: Home Realm Discovery (cloud
/// only, PUBLIC -- no `admin_guard`). Given an email, resolves its domain to
/// a verified SSO email domain and returns the owning tenant's slug so the
/// login UI can send the user straight to `/admin/login?org=<slug>`.
///
/// Routes ONLY on a `Verified` domain whose tenant still has an
/// `oidc_config` and a resolvable slug; every other outcome (missing/
/// malformed email, unknown domain, still-`Pending`, or a tenant that lost
/// its `oidc_config`/no longer exists) returns the same empty `{}` -- never a
/// 404 or a distinguishable error, so this endpoint cannot be used to probe
/// which domains or tenants exist.
pub(crate) async fn sso_discover(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<DiscoverParams>,
) -> Response {
    if !st.multi_tenant {
        return StatusCode::NOT_FOUND.into_response();
    }
    let ip = client_ip(&headers, &st.real_ip_header, None);
    if !st.ratelimiter.check(&ip, now()).await {
        return (StatusCode::TOO_MANY_REQUESTS, "too many requests").into_response();
    }
    let empty = || Json(DiscoverResp::default()).into_response();
    let Some(email) = params.email else {
        return empty();
    };
    let Some(domain) = normalize_email_domain(&email) else {
        return empty();
    };
    let row = match st.store.get_sso_domain_bare(&domain).await {
        Ok(Some(r)) => r,
        Ok(None) => return empty(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    if row.status != DomainStatus::Verified {
        return empty();
    }
    match st.store.get_oidc_config_bare(row.tenant_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return empty(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    match st.store.get_tenant(row.tenant_id).await {
        Ok(Some(tenant)) => Json(DiscoverResp {
            org: Some(tenant.slug),
        })
        .into_response(),
        Ok(None) => empty(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

// --- Team invites (multi-tenancy P2c), cloud-only ---
