//! Seam for per-tenant identity providers (LUC-145).
//!
//! Signing in against a tenant's *own* IdP is an Enterprise feature: the config
//! lives in `oidc_configs`, it is administered by `/admin/oidc-config`, and the
//! realm behind it is provisioned by the Keycloak code. All of that is in
//! `src/ee/`. What stays in the core is single-issuer OIDC login, configured
//! with `QUARK_OIDC_ISSUER` (see D2.1 in
//! `docs/specs/2026-08-03-luc19-open-core-design.md`).
//!
//! `api/oidc_login.rs` serves both, so this module is the line between them.
//! The Community build gets implementations that resolve nothing, which is
//! exactly the behavior the `?org=` arm already had there: a 404 that does not
//! distinguish an unknown organization from a real one without its own IdP.
//!
//! It is two `cfg`-selected functions rather than a trait object on purpose.
//! The choice is made at compile time and never varies at runtime, so a trait
//! would buy a vtable and an `Arc` in `AppState` for no polymorphism at all.

#[cfg(not(feature = "ee"))]
use super::AppState;
#[cfg(not(feature = "ee"))]
use crate::oidc::{OidcRuntime, TenantOidcConfig};
#[cfg(not(feature = "ee"))]
use crate::tenant::TenantId;
#[cfg(not(feature = "ee"))]
use axum::http::HeaderMap;
#[cfg(not(feature = "ee"))]
use std::sync::Arc;

/// Why a per-tenant IdP could not be resolved. Callers map these to status
/// codes; the mapping differs by caller, which is why this carries the reason
/// instead of a `Response`.
// The Community build matches on every variant but constructs none of them:
// its resolvers only ever return `NotFound`. The variants are still part of the
// contract the Enterprise implementation fills, so they stay.
#[cfg_attr(not(feature = "ee"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TenantIdpError {
    /// Unknown slug, unknown tenant, or a tenant with no IdP of its own.
    /// Deliberately one variant and not three: telling them apart in a
    /// response would let an unauthenticated caller enumerate organizations.
    NotFound,
    /// The store could not answer. Maps to `503`.
    Unavailable,
    /// The tenant's IdP could not be reached or its metadata was unusable.
    /// Maps to `502`.
    Unreachable,
    /// The per-IP brake on slug resolution tripped. Maps to `429`.
    RateLimited,
}

/// Resolves the runtime for a tenant named by URL slug (`/admin/login?org=`),
/// applying the per-IP rate limit that keeps slug existence from leaking
/// through response timing.
///
/// Community: always `NotFound`.
#[cfg(not(feature = "ee"))]
pub(crate) async fn runtime_for_slug(
    _st: &AppState,
    _slug: &str,
    _headers: &HeaderMap,
) -> Result<(Arc<OidcRuntime>, TenantId), TenantIdpError> {
    Err(TenantIdpError::NotFound)
}

/// Resolves the runtime and config for a tenant already known by id, used by
/// the callback (the tenant comes from the signed login cookie) and by logout.
///
/// Community: always `NotFound`.
#[cfg(not(feature = "ee"))]
pub(crate) async fn runtime_for_tenant(
    _st: &AppState,
    _tenant: TenantId,
) -> Result<(Arc<OidcRuntime>, TenantOidcConfig), TenantIdpError> {
    Err(TenantIdpError::NotFound)
}

#[cfg(feature = "ee")]
pub(crate) use crate::ee::api::tenant_idp::{runtime_for_slug, runtime_for_tenant};
