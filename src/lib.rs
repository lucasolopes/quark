pub mod abuse;
pub mod analytics;
pub mod api;
pub mod cache;
pub mod codec;
pub mod permute;
pub mod store;

use std::time::{SystemTime, UNIX_EPOCH};

/// Epoch em segundos (UTC). Saturating em 0 se o relógio estiver antes de 1970.
/// Ponto único usado pelo caminho de request (`api`) e pelo cache (TTL de L2).
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
