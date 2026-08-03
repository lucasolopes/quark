//! Per-tenant identity provider resolution (LUC-145). Covered by
//! `src/ee/LICENSE`, not by the AGPL.
//!
//! The Enterprise half of `api/oidc_login.rs`: given a tenant slug or id,
//! produce the `OidcRuntime` built from that tenant's own `oidc_configs` row.
//! The core keeps single-issuer login and calls in here through
//! `crate::api::tenant_idp`, whose Community implementations resolve nothing.

use crate::api::tenant_idp::TenantIdpError;
use crate::api::{client_ip, AppState};
use crate::now;
use crate::oidc::{OidcRuntime, TenantOidcConfig};
use crate::tenant::TenantId;
use axum::http::HeaderMap;
use std::sync::Arc;

/// Slug to runtime, for `GET /admin/login?org=<slug>`.
///
/// Rate-limited by IP (LUC-51): the generic 404 already hides slug existence
/// in the body, but an unthrottled caller could still probe slugs by response
/// timing, since an unknown slug costs one query and a configured one costs
/// more. A per-IP brake closes that side channel.
///
/// Unknown slug and "known tenant, no IdP of its own" both return `NotFound`,
/// and the caller renders the identical body for each: an unauthenticated
/// caller must not be able to tell a nonexistent organization from a real one
/// that simply has not set up OIDC.
pub(crate) async fn runtime_for_slug(
    st: &AppState,
    slug: &str,
    headers: &HeaderMap,
) -> Result<(Arc<OidcRuntime>, TenantId), TenantIdpError> {
    let ip = client_ip(headers, &st.real_ip_header, None);
    if !st.ratelimiter.check(&ip, now()).await {
        return Err(TenantIdpError::RateLimited);
    }
    let tenant = match st.store.get_tenant_by_slug(slug).await {
        Ok(Some(t)) => t,
        Ok(None) => return Err(TenantIdpError::NotFound),
        Err(_) => return Err(TenantIdpError::Unavailable),
    };
    let (rt, _) = runtime_for_tenant(st, tenant.id).await?;
    Ok((rt, tenant.id))
}

/// Tenant id to runtime and config, for the callback (the tenant comes from
/// the signed login cookie, never from a request parameter) and for logout.
pub(crate) async fn runtime_for_tenant(
    st: &AppState,
    tenant: TenantId,
) -> Result<(Arc<OidcRuntime>, TenantOidcConfig), TenantIdpError> {
    let cfg = match st.store.get_oidc_config_bare(tenant).await {
        Ok(Some(c)) => c,
        Ok(None) => return Err(TenantIdpError::NotFound),
        Err(_) => return Err(TenantIdpError::Unavailable),
    };
    match st.ee.oidc_tenants.get_or_build(tenant, &cfg).await {
        Ok(rt) => Ok((rt, cfg)),
        Err(_) => Err(TenantIdpError::Unreachable),
    }
}
