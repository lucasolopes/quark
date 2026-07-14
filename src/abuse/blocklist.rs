use crate::store::Store;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;

const VALKEY_KEY: &str = "quark:blocklist";

struct Snapshot {
    loaded_at: u64,
    set: HashSet<String>,
}

/// Domain blocklist with an in-memory snapshot (L1) over the `Store`, and an
/// optional Valkey (L2) as a shared source across replicas. Fail-open:
/// a Valkey error falls back to the `Store`. Propagation across replicas is eventual (<= TTL).
pub struct Blocklist {
    store: Arc<dyn Store>,
    valkey: Option<redis::aio::MultiplexedConnection>,
    ttl_secs: u64,
    snap: RwLock<Snapshot>,
}

impl Blocklist {
    pub fn new(
        store: Arc<dyn Store>,
        valkey: Option<redis::aio::MultiplexedConnection>,
        ttl_secs: u64,
    ) -> Blocklist {
        Blocklist {
            store,
            valkey,
            ttl_secs,
            snap: RwLock::new(Snapshot {
                loaded_at: 0,
                set: HashSet::new(),
            }),
        }
    }

    pub async fn is_blocked(&self, host: &str, now_secs: u64) -> bool {
        self.ensure_fresh(now_secs).await;
        let snap = self.snap.read().await;
        super::host_in_blocklist(host, &snap.set)
    }

    /// Forces a reload on the next check and deletes the shared Valkey key.
    pub async fn invalidate(&self) {
        {
            let mut snap = self.snap.write().await;
            snap.loaded_at = 0;
        }
        if let Some(conn) = &self.valkey {
            let mut c = conn.clone();
            let _: Result<(), _> = redis::cmd("DEL").arg(VALKEY_KEY).query_async(&mut c).await;
        }
    }

    async fn ensure_fresh(&self, now_secs: u64) {
        {
            let snap = self.snap.read().await;
            if snap.loaded_at != 0 && now_secs.saturating_sub(snap.loaded_at) < self.ttl_secs {
                return;
            }
        }
        let set = self.load_set().await;
        let mut snap = self.snap.write().await;
        snap.set = set;
        snap.loaded_at = now_secs.max(1);
    }

    /// Loads the set: tries Valkey (L2); if absent/error, reads the Store and
    /// populates Valkey best-effort.
    async fn load_set(&self) -> HashSet<String> {
        if let Some(conn) = &self.valkey {
            let mut c = conn.clone();
            let cached: Result<Option<String>, _> =
                redis::cmd("GET").arg(VALKEY_KEY).query_async(&mut c).await;
            if let Ok(Some(json)) = cached {
                if let Ok(v) = serde_json::from_str::<Vec<String>>(&json) {
                    return v.into_iter().collect();
                }
            }
        }
        let list = self.store.list_blocked_domains().await.unwrap_or_default();
        if let Some(conn) = &self.valkey {
            if let Ok(json) = serde_json::to_string(&list) {
                let mut c = conn.clone();
                let _: Result<(), _> = redis::cmd("SET")
                    .arg(VALKEY_KEY)
                    .arg(json)
                    .query_async(&mut c)
                    .await;
            }
        }
        list.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::Blocklist;
    use crate::store::{lmdb::LmdbStore, Store};
    use std::sync::Arc;

    #[tokio::test]
    async fn reflects_the_store_and_matches_subdomain() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn Store> =
            Arc::new(LmdbStore::open_with_node_id(dir.path(), None).unwrap());
        store.add_blocked_domain("evil.com").await.unwrap();

        let bl = Blocklist::new(store.clone(), None, 60);
        assert!(bl.is_blocked("evil.com", 100).await);
        assert!(bl.is_blocked("x.evil.com", 100).await);
        assert!(!bl.is_blocked("ok.com", 100).await);
    }

    #[tokio::test]
    async fn invalidate_forces_reload() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn Store> =
            Arc::new(LmdbStore::open_with_node_id(dir.path(), None).unwrap());
        let bl = Blocklist::new(store.clone(), None, 3600);

        assert!(!bl.is_blocked("late.com", 100).await);
        store.add_blocked_domain("late.com").await.unwrap();
        assert!(!bl.is_blocked("late.com", 101).await);
        bl.invalidate().await;
        assert!(bl.is_blocked("late.com", 102).await);
    }

    #[tokio::test]
    async fn reloads_after_ttl() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn Store> =
            Arc::new(LmdbStore::open_with_node_id(dir.path(), None).unwrap());
        let bl = Blocklist::new(store.clone(), None, 10);
        assert!(!bl.is_blocked("z.com", 100).await);
        store.add_blocked_domain("z.com").await.unwrap();
        assert!(!bl.is_blocked("z.com", 105).await);
        assert!(bl.is_blocked("z.com", 111).await);
    }
}
