use super::*;

/// Name of the TXT record a caller must publish to prove ownership of a
/// custom domain.
pub(crate) fn verify_txt_name(host: &str) -> String {
    format!("_quark-verify.{host}")
}

/// A minimal syntax check for a host a caller wants to bind: dotted labels,
/// each 1-63 characters of alphanumerics/hyphens, no leading/trailing hyphen.
/// Not a full RFC 1035 validator, just enough to reject obvious junk before
/// it reaches the store.
pub(crate) fn is_valid_host_format(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 || !host.contains('.') {
        return false;
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    })
}

#[derive(Deserialize)]
pub(crate) struct CreateDomainReq {
    host: String,
}

/// A domain plus the DNS instructions to verify it: the panel and any caller
/// need both together, so `list`/`create`/`verify` all return this shape
/// rather than the bare store `Domain`.
#[derive(Serialize)]
pub(crate) struct DomainView {
    id: u64,
    host: String,
    status: DomainStatus,
    created: u64,
    verified_at: Option<u64>,
    /// Name of the TXT record to publish: `_quark-verify.<host>`.
    txt_name: String,
    /// Value the TXT record must hold (the domain's verification token).
    txt_value: String,
    /// CNAME target the caller should point `host` at. `None` when this
    /// deploy has no shared `public_host` configured.
    cname_target: Option<String>,
    /// True when this is the tenant's primary link domain (LUC-86): the one the
    /// copy button and new links use by default.
    primary: bool,
}

pub(crate) fn domain_view(d: &Domain, public_host: &Option<String>, primary: bool) -> DomainView {
    DomainView {
        id: d.id,
        host: d.host.clone(),
        status: d.status,
        created: d.created,
        verified_at: d.verified_at,
        txt_name: verify_txt_name(&d.host),
        txt_value: d.token.clone(),
        cname_target: public_host.clone(),
        primary,
    }
}

/// `GET /admin/domains`: list the caller's tenant's custom domains, each with
/// the DNS instructions needed to verify it (cloud only).
pub(crate) async fn admin_domains_list(
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
    let primary_id = st
        .store
        .get_primary_domain_id(p.tenant)
        .await
        .ok()
        .flatten();
    match st.store.list_domains(p.tenant).await {
        Ok(domains) => Json(
            domains
                .iter()
                .map(|d| domain_view(d, &st.public_host, Some(d.id) == primary_id))
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// `POST /admin/domains {host}`: register a custom domain for the caller's
/// tenant, pending DNS verification (cloud only). Rejects internal hosts and
/// the shared `public_host`, and normalizes `host` (lowercase, trimmed, no
/// trailing dot) before storing — `get_domain_by_host`/`HostRouter` always
/// query lowercase, so an un-normalized host would never resolve. Duplicate
/// `host` (unique violation) -> 409.
pub(crate) async fn admin_domains_create(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateDomainReq>,
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
    let host = req.host.trim().trim_end_matches('.').to_ascii_lowercase();
    if !is_valid_host_format(&host) {
        return (StatusCode::BAD_REQUEST, "invalid host").into_response();
    }
    if is_internal_host(&host) || st.public_host.as_deref() == Some(host.as_str()) {
        return (StatusCode::BAD_REQUEST, "host not allowed").into_response();
    }
    let id = match st.store.next_domain_id().await {
        Ok(i) => i,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let domain = Domain {
        id,
        tenant_id: p.tenant,
        host,
        token: generate_token(),
        status: DomainStatus::Pending,
        created: now(),
        verified_at: None,
    };
    if let Err(e) = st.store.put_domain(&domain).await {
        return conflict_or_503(e).into_response();
    }
    // A freshly created domain is pending and never the primary yet.
    Json(domain_view(&domain, &st.public_host, false)).into_response()
}

/// `POST /admin/domains/:id/verify`: look up the `_quark-verify.<host>` TXT
/// record for the caller's tenant's domain; on a match, mark it `Verified`
/// and invalidate the host router so the new route takes effect immediately.
/// A missing or mismatched TXT record leaves the domain `Pending`.
pub(crate) async fn admin_domains_verify(
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
    let ip = client_ip(&headers, &st.real_ip_header, None);
    if !st.ratelimiter.check(&ip, now()).await {
        return (StatusCode::TOO_MANY_REQUESTS, "too many requests").into_response();
    }
    let mut domain = match st.store.get_domain(p.tenant, id).await {
        Ok(Some(d)) => d,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let txt_name = verify_txt_name(&domain.host);
    let matched = st
        .dns
        .lookup_txt(&txt_name)
        .await
        .map(|values| values.iter().any(|v| v == &domain.token))
        .unwrap_or(false);
    if matched {
        let verified_at = now();
        if st
            .store
            .set_domain_status(p.tenant, id, DomainStatus::Verified, Some(verified_at))
            .await
            .is_err()
        {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        st.host_router.invalidate(&domain.host).await;
        domain.status = DomainStatus::Verified;
        domain.verified_at = Some(verified_at);
    }
    let primary_id = st
        .store
        .get_primary_domain_id(p.tenant)
        .await
        .ok()
        .flatten();
    Json(domain_view(
        &domain,
        &st.public_host,
        Some(domain.id) == primary_id,
    ))
    .into_response()
}

/// `DELETE /admin/domains/:id`: remove the caller's tenant's custom domain
/// and drop any cached host-router entry for it (cloud only).
pub(crate) async fn admin_domains_delete(
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
    let domain = match st.store.get_domain(p.tenant, id).await {
        Ok(Some(d)) => d,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    if st.store.delete_domain(p.tenant, id).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    // If the deleted domain was the tenant's primary, clear the pointer so the
    // default falls back to the subdomain (a dangling primary is harmless — the
    // resolvers already ignore it — but clearing keeps the row honest).
    if st
        .store
        .get_primary_domain_id(p.tenant)
        .await
        .ok()
        .flatten()
        == Some(id)
    {
        let _ = st.store.set_primary_domain(p.tenant, None).await;
    }
    st.host_router.invalidate(&domain.host).await;
    StatusCode::NO_CONTENT.into_response()
}

/// `POST /admin/domains/:id/primary`: make the caller's tenant's domain its
/// primary link domain (LUC-86) — the one the copy button and new links use by
/// default (cloud only, Owner/Admin). The domain must exist and be `Verified`;
/// setting it replaces any previous primary (stored as a single pointer on the
/// tenant row).
pub(crate) async fn admin_domains_set_primary(
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
    let domain = match st.store.get_domain(p.tenant, id).await {
        Ok(Some(d)) => d,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    if domain.status != DomainStatus::Verified {
        return (StatusCode::BAD_REQUEST, "domain must be verified first").into_response();
    }
    if st
        .store
        .set_primary_domain(p.tenant, Some(id))
        .await
        .is_err()
    {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    Json(domain_view(&domain, &st.public_host, true)).into_response()
}

// --- SSO email-domain discovery (LUC-57 Task 2), cloud-only ---
