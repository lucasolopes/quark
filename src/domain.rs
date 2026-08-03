//! Custom domain model (multi-tenancy P3): a tenant can bind its own host
//! (e.g. `go.acme.com`) to redirect through quark instead of the shared
//! domain. `get_domain_by_host` is the one public, cross-tenant lookup: the
//! redirect handler sees only a `Host` header before it knows which tenant
//! owns it, so that path runs on the bare pool, unscoped.
use crate::tenant::TenantId;
use serde::{Deserialize, Serialize};

/// Sentinel tenant id used where a domain concept needs "no custom domain,
/// the shared one applies" without an `Option`. Distinct from
/// `DEFAULT_TENANT` (also `0`) in meaning, not in value.
pub const SHARED_DOMAIN_ID: u64 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DomainStatus {
    Pending,
    Verified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Domain {
    pub id: u64,
    pub tenant_id: TenantId,
    pub host: String,
    pub token: String,
    pub status: DomainStatus,
    pub created: u64,
    pub verified_at: Option<u64>,
}

/// Result of a public host lookup: the minimal binding needed to route a
/// redirect (which tenant owns this host), without dragging along the full
/// verification record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainRoute {
    pub domain_id: u64,
    pub tenant_id: TenantId,
}

/// The host of a tenant's automatic subdomain (multi-tenancy P3-completion),
/// e.g. `subdomain_host("acme", "quarkus.com.br") == "acme.quarkus.com.br"`.
/// Lowercased so it matches the lookup convention every other host in
/// `domains` follows (`get_domain_by_host`/`HostRouter` always query lowercase).
///
/// Stays in the core rather than `src/ee/` because `api/links.rs` builds the
/// short URL with it (LUC-19: the hot path wins). Administering the subdomain is
/// the part that is Enterprise.
pub fn subdomain_host(slug: &str, suffix: &str) -> String {
    format!("{slug}.{suffix}").to_ascii_lowercase()
}
