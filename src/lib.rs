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
pub mod oidc;
pub mod password;
pub mod permute;
pub mod pixel;
pub mod sheets;
pub mod store;
pub mod tenant;
pub mod webhooks;

use std::time::{SystemTime, UNIX_EPOCH};

/// Epoch in seconds (UTC). Saturates to 0 if the clock is before 1970.
/// Single point used by the request path (`api`) and by the cache (L2 TTL).
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
