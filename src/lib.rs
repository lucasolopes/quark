pub mod abuse;
pub mod analytics;
pub mod api;
pub mod auth;
pub mod cache;
pub mod cluster;
pub mod codec;
pub mod dns;
pub mod domain;
pub mod domain_router;
pub mod health;
pub mod import;
pub mod invalidate;
pub mod invite;
pub mod keycloak;
pub mod oidc;
pub mod password;
pub mod permute;
pub mod pixel;
pub mod secretbox;
pub mod sheets;
pub mod slack;
pub mod sso;
pub mod store;
pub mod tenant;
pub mod webhooks;

use std::time::{SystemTime, UNIX_EPOCH};

/// TCP+TLS connect budget for every outbound HTTP client, always smaller than
/// the client's total request timeout (the smallest total in the crate is 5s).
///
/// Without a separate connect timeout, a destination that accepts the TCP
/// connection and then stalls during the TLS handshake consumes the whole
/// request budget. That matters most for the webhook and pixel clients, which
/// fetch user-supplied URLs and share the runtime with the redirect hot path.
/// Shared here because every module needs the same figure.
pub const HTTP_CONNECT_TIMEOUT_SECS: u64 = 3;

/// How long a worker's full snapshot refresh (`list_tenants` plus a per-tenant
/// listing) may run before it is abandoned in favor of the previous snapshot.
///
/// Fail-open budget: a wedged store must never stall a worker, and keeping a
/// good previous snapshot beats zeroing it over a transient error. Shared
/// because the analytics and webhook workers had the same 3s under two names
/// (`PIXEL_SNAPSHOT_TIMEOUT` and `SNAPSHOT_TIMEOUT`), which meant changing the
/// budget took two edits.
pub const SNAPSHOT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Epoch in seconds (UTC). Saturates to 0 if the clock is before 1970.
/// Single point used by the request path (`api`) and by the cache (L2 TTL).
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Lowercase hex encoding of a byte slice (two `%02x` digits per byte).
/// Shared by id generators, digests, and lease holder ids across the crate.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
