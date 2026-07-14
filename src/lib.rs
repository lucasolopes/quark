pub mod abuse;
pub mod analytics;
pub mod api;
pub mod auth;
pub mod cache;
pub mod codec;
pub mod permute;
pub mod store;

use std::time::{SystemTime, UNIX_EPOCH};

/// Epoch in seconds (UTC). Saturates to 0 if the clock is before 1970.
/// Single point used by the request path (`api`) and by the cache (L2 TTL).
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
