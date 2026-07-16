pub mod valkey;

use crate::invalidate::Invalidator;
use crate::now;
use crate::store::{Record, Store, StoreError};
use moka::sync::Cache as Moka;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

pub const BREAKER_THRESHOLD: u32 = 5;
pub const BREAKER_COOLDOWN_SECS: u64 = 30;
pub const L1_TTL_SECS: u64 = 60;
pub const L2_TTL_SECS: u64 = 3600;
/// Timeout per L2 operation: a Valkey that accepts the connection but stops
/// responding (overload/black-hole/pause) must never block the redirect —
/// that would break the sacred invariant. A timeout counts as a failure for
/// the breaker, same as a tier `Err`.
pub const L2_OP_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(100);

#[derive(Debug)]
pub struct TierError(pub String);
impl std::fmt::Display for TierError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "tier: {}", self.0)
    }
}
impl std::error::Error for TierError {}

/// Pluggable L2 (network) layer. Implemented by `ValkeyTier` (Valkey/Redis) and
/// by the fake tiers in the tests. Tier errors never propagate to the caller:
/// `Cache::get` records them in the `Breaker` and falls back to the store.
#[async_trait::async_trait]
pub trait CacheTier: Send + Sync + 'static {
    async fn get(&self, id: u64) -> Result<Option<Record>, TierError>;
    async fn set(&self, id: u64, rec: &Record, ttl_secs: u64) -> Result<(), TierError>;
    async fn invalidate(&self, id: u64) -> Result<(), TierError>;
}

/// Effective L2 TTL for a record: capped by the default TTL and by the time
/// remaining until the link's expiry (no point caching past validity).
pub fn l2_ttl(rec: &Record, now: u64, l2_ttl_secs: u64) -> u64 {
    match rec.expiry {
        Some(e) if e > now => (e - now).min(l2_ttl_secs),
        Some(_) => 0,
        None => l2_ttl_secs,
    }
}

/// Simple circuit breaker via atomics (no locks): opens after
/// `BREAKER_THRESHOLD` consecutive failures, allows traffic again (half-open)
/// after `BREAKER_COOLDOWN_SECS`. A failure while half-open reopens it.
struct Breaker {
    failures: AtomicU32,
    opened_at: AtomicU64,
}
impl Breaker {
    fn new() -> Breaker {
        Breaker {
            failures: AtomicU32::new(0),
            opened_at: AtomicU64::new(0),
        }
    }
    /// Should the L2 be queried now?
    fn allow(&self, now: u64) -> bool {
        let opened = self.opened_at.load(Ordering::Relaxed);
        if opened == 0 {
            return true;
        }
        now.saturating_sub(opened) >= BREAKER_COOLDOWN_SECS
    }
    fn record_success(&self) {
        self.failures.store(0, Ordering::Relaxed);
        self.opened_at.store(0, Ordering::Relaxed);
    }
    fn record_failure(&self, now: u64) {
        let f = self.failures.fetch_add(1, Ordering::Relaxed) + 1;
        if f >= BREAKER_THRESHOLD {
            self.opened_at.store(now, Ordering::Relaxed);
        }
    }
}

pub struct Cache {
    store: Arc<dyn Store>,
    hot: Moka<u64, Record>,
    l2: Option<Arc<dyn CacheTier>>,
    l2_ttl_secs: u64,
    breaker: Breaker,
    invalidator: Option<Arc<Invalidator>>,
}

impl Cache {
    pub fn new(
        store: Arc<dyn Store>,
        capacity: u64,
        invalidator: Option<Arc<Invalidator>>,
    ) -> Cache {
        Cache::build(store, capacity, None, L1_TTL_SECS, L2_TTL_SECS, invalidator)
    }

    pub fn with_l2(
        store: Arc<dyn Store>,
        capacity: u64,
        l2: Arc<dyn CacheTier>,
        l1_ttl_secs: u64,
        l2_ttl_secs: u64,
        invalidator: Option<Arc<Invalidator>>,
    ) -> Cache {
        Cache::build(
            store,
            capacity,
            Some(l2),
            l1_ttl_secs,
            l2_ttl_secs,
            invalidator,
        )
    }

    fn build(
        store: Arc<dyn Store>,
        capacity: u64,
        l2: Option<Arc<dyn CacheTier>>,
        l1_ttl: u64,
        l2_ttl: u64,
        invalidator: Option<Arc<Invalidator>>,
    ) -> Cache {
        let hot = Moka::builder()
            .max_capacity(capacity)
            .time_to_live(std::time::Duration::from_secs(l1_ttl))
            .build();
        Cache {
            store,
            hot,
            l2,
            l2_ttl_secs: l2_ttl,
            breaker: Breaker::new(),
            invalidator,
        }
    }

    pub async fn get(&self, id: u64) -> Result<Option<Record>, StoreError> {
        if let Some(rec) = self.hot.get(&id) {
            return Ok(Some(rec));
        }
        let n = now();
        let mut l2_failed_this_request = false;
        if let Some(l2) = &self.l2 {
            if self.breaker.allow(n) {
                match tokio::time::timeout(L2_OP_TIMEOUT, l2.get(id)).await {
                    Ok(Ok(Some(rec))) => {
                        self.breaker.record_success();
                        self.hot.insert(id, rec.clone());
                        return Ok(Some(rec));
                    }
                    Ok(Ok(None)) => {
                        self.breaker.record_success();
                    }
                    Ok(Err(_)) | Err(_) => {
                        self.breaker.record_failure(n);
                        l2_failed_this_request = true;
                    }
                }
            }
        }
        match self
            .store
            .get_link(crate::tenant::DEFAULT_TENANT, id)
            .await?
        {
            Some(rec) => {
                if let Some(l2) = &self.l2 {
                    if !l2_failed_this_request && self.breaker.allow(n) {
                        let ttl = l2_ttl(&rec, n, self.l2_ttl_secs);
                        if ttl > 0 {
                            match tokio::time::timeout(L2_OP_TIMEOUT, l2.set(id, &rec, ttl)).await {
                                Ok(Ok(())) => self.breaker.record_success(),
                                Ok(Err(_)) | Err(_) => self.breaker.record_failure(n),
                            }
                        }
                    }
                }
                self.hot.insert(id, rec.clone());
                Ok(Some(rec))
            }
            None => Ok(None),
        }
    }

    /// Removes an id from the cache: L1 always; L2 best-effort (breaker + timeout), like
    /// the other tier ops. Used when a link is edited or deleted, so the
    /// redirect stops serving the old value.
    pub async fn invalidate(&self, id: u64) {
        self.hot.invalidate(&id);
        if let Some(l2) = &self.l2 {
            let n = now();
            if self.breaker.allow(n) {
                match tokio::time::timeout(L2_OP_TIMEOUT, l2.invalidate(id)).await {
                    Ok(Ok(())) => self.breaker.record_success(),
                    Ok(Err(_)) | Err(_) => self.breaker.record_failure(n),
                }
            }
        }
        if let Some(inv) = &self.invalidator {
            inv.publish(&format!("link:{id}")).await;
        }
    }

    /// Drops only the L1 (moka) entry for `id`, without touching L2 or
    /// publishing. Called by the pub/sub subscriber when another replica
    /// invalidated this id: it clears the local stale copy and must NOT
    /// re-publish (that would loop across the cluster).
    pub async fn invalidate_local(&self, id: u64) {
        self.hot.invalidate(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{lmdb::LmdbStore, Record, Store};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    fn rec(url: &str) -> Record {
        Record {
            url: url.into(),
            expiry: None,
            created: 0,
            tags: Vec::new(),
            max_visits: None,
            rules: Vec::new(),
            variants: Vec::new(),
            app_ios: None,
            app_android: None,
            folder: None,
            fallback_url: None,
            password_hash: None,
        }
    }

    struct FailingTier {
        calls: AtomicU32,
    }
    #[async_trait::async_trait]
    impl CacheTier for FailingTier {
        async fn get(&self, _id: u64) -> Result<Option<Record>, TierError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(TierError("down".into()))
        }
        async fn set(&self, _id: u64, _r: &Record, _t: u64) -> Result<(), TierError> {
            Err(TierError("down".into()))
        }
        async fn invalidate(&self, _id: u64) -> Result<(), TierError> {
            Err(TierError("down".into()))
        }
    }

    struct HangingTier;
    #[async_trait::async_trait]
    impl CacheTier for HangingTier {
        async fn get(&self, _id: u64) -> Result<Option<Record>, TierError> {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            Ok(None)
        }
        async fn set(&self, _id: u64, _r: &Record, _t: u64) -> Result<(), TierError> {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            Ok(())
        }
        async fn invalidate(&self, _id: u64) -> Result<(), TierError> {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            Ok(())
        }
    }

    #[tokio::test(start_paused = true)]
    async fn hanging_l2_falls_back_to_store_via_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn Store> = Arc::new(LmdbStore::open(dir.path()).unwrap());
        store
            .put_link(crate::tenant::DEFAULT_TENANT, 9, &rec("hung"))
            .await
            .unwrap();
        let c = Cache::with_l2(store, 1000, Arc::new(HangingTier), 60, 3600, None);
        let got = c.get(9).await.unwrap().unwrap();
        assert_eq!(got.url, "hung");
    }

    #[tokio::test]
    async fn invalidate_removes_from_l1() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn Store> = Arc::new(LmdbStore::open(dir.path()).unwrap());
        store
            .put_link(crate::tenant::DEFAULT_TENANT, 1, &rec("u1"))
            .await
            .unwrap();
        let c = Cache::new(store.clone(), 1000, None);
        assert_eq!(c.get(1).await.unwrap().unwrap().url, "u1");
        store
            .delete_link(crate::tenant::DEFAULT_TENANT, 1)
            .await
            .unwrap();
        c.invalidate(1).await;
        assert!(c.get(1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn invalidate_local_removes_from_l1() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn Store> = Arc::new(LmdbStore::open(dir.path()).unwrap());
        store
            .put_link(crate::tenant::DEFAULT_TENANT, 2, &rec("u2"))
            .await
            .unwrap();
        let c = Cache::new(store.clone(), 1000, None);
        assert_eq!(c.get(2).await.unwrap().unwrap().url, "u2");
        store
            .delete_link(crate::tenant::DEFAULT_TENANT, 2)
            .await
            .unwrap();
        c.invalidate_local(2).await;
        assert!(c.get(2).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn without_l2_behaves_as_today() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn Store> = Arc::new(LmdbStore::open(dir.path()).unwrap());
        store
            .put_link(crate::tenant::DEFAULT_TENANT, 3, &rec("u"))
            .await
            .unwrap();
        let c = Cache::new(store, 1000, None);
        assert_eq!(c.get(3).await.unwrap().unwrap().url, "u");
        assert!(c.get(404).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn l2_down_falls_back_to_store_and_opens_breaker() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn Store> = Arc::new(LmdbStore::open(dir.path()).unwrap());
        store
            .put_link(crate::tenant::DEFAULT_TENANT, 7, &rec("v"))
            .await
            .unwrap();
        let tier = Arc::new(FailingTier {
            calls: AtomicU32::new(0),
        });
        let c = Cache::with_l2(store, 1000, tier.clone(), 60, 3600, None);
        for _ in 0..7 {
            let _ = c.get(7).await.unwrap();
        }
        for id in 100..110u64 {
            let _ = c.get(id).await.unwrap();
        }
        let calls = tier.calls.load(Ordering::SeqCst);
        assert!(
            calls >= BREAKER_THRESHOLD,
            "should have tried the L2 at least the threshold amount"
        );
        assert!(
            calls <= BREAKER_THRESHOLD + 2,
            "after opening, stops calling the L2 (calls={calls})"
        );
    }

    #[test]
    fn l2_ttl_capped_by_expiry() {
        let now = 1000u64;
        assert_eq!(
            l2_ttl(
                &Record {
                    url: "".into(),
                    expiry: None,
                    created: 0,
                    tags: Vec::new(),
                    max_visits: None,
                    rules: Vec::new(),
                    variants: Vec::new(),
                    app_ios: None,
                    app_android: None,
                    folder: None,
                    fallback_url: None,
                    password_hash: None
                },
                now,
                3600
            ),
            3600
        );
        assert_eq!(
            l2_ttl(
                &Record {
                    url: "".into(),
                    expiry: Some(now + 100),
                    created: 0,
                    tags: Vec::new(),
                    max_visits: None,
                    rules: Vec::new(),
                    variants: Vec::new(),
                    app_ios: None,
                    app_android: None,
                    folder: None,
                    fallback_url: None,
                    password_hash: None
                },
                now,
                3600
            ),
            100
        );
        assert_eq!(
            l2_ttl(
                &Record {
                    url: "".into(),
                    expiry: Some(now + 999_999),
                    created: 0,
                    tags: Vec::new(),
                    max_visits: None,
                    rules: Vec::new(),
                    variants: Vec::new(),
                    app_ios: None,
                    app_android: None,
                    folder: None,
                    fallback_url: None,
                    password_hash: None
                },
                now,
                3600
            ),
            3600
        );
        assert_eq!(
            l2_ttl(
                &Record {
                    url: "".into(),
                    expiry: Some(now - 1),
                    created: 0,
                    tags: Vec::new(),
                    max_visits: None,
                    rules: Vec::new(),
                    variants: Vec::new(),
                    app_ios: None,
                    app_android: None,
                    folder: None,
                    fallback_url: None,
                    password_hash: None
                },
                now,
                3600
            ),
            0
        );
    }

    #[test]
    fn breaker_reopens_after_half_open_probe_fails() {
        let b = Breaker::new();
        let t0 = 1_000_000u64;
        assert!(b.allow(t0));
        for _ in 0..BREAKER_THRESHOLD {
            b.record_failure(t0);
        }
        assert!(!b.allow(t0));
        assert!(!b.allow(t0 + BREAKER_COOLDOWN_SECS - 1));
        let t1 = t0 + BREAKER_COOLDOWN_SECS;
        assert!(b.allow(t1));
        b.record_failure(t1);
        assert!(!b.allow(t1));
        assert!(!b.allow(t1 + BREAKER_COOLDOWN_SECS - 1));
        b.record_success();
        assert!(b.allow(t1 + 1));
    }
}
