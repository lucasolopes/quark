// src/abuse/blocklist.rs — implementado na Task 4

use crate::store::Store;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;

const VALKEY_KEY: &str = "quark:blocklist";

struct Snapshot {
    loaded_at: u64, // epoch secs; 0 = nunca / inválido
    set: HashSet<String>,
}

/// Blocklist de domínios com snapshot em memória (L1) sobre o `Store`, e
/// Valkey opcional (L2) como fonte compartilhada entre réplicas. Fail-open:
/// erro de Valkey cai para o `Store`. Propagação entre réplicas é eventual (≤ TTL).
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

    /// Força recarga na próxima checagem e apaga a chave compartilhada do Valkey.
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
                return; // ainda fresco
            }
        }
        let set = self.load_set().await;
        let mut snap = self.snap.write().await;
        snap.set = set;
        snap.loaded_at = now_secs.max(1); // nunca 0 (0 = inválido)
    }

    /// Carrega o conjunto: tenta Valkey (L2); se ausente/erro, lê o Store e
    /// popula o Valkey best-effort.
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
        // fonte da verdade: o Store
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
    async fn reflete_o_store_e_casa_subdominio() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn Store> =
            Arc::new(LmdbStore::open_with_node_id(dir.path(), None).unwrap());
        store.add_blocked_domain("evil.com").await.unwrap();

        let bl = Blocklist::new(store.clone(), None, 60);
        // t=100: primeira checagem carrega o snapshot
        assert!(bl.is_blocked("evil.com", 100).await);
        assert!(bl.is_blocked("x.evil.com", 100).await);
        assert!(!bl.is_blocked("ok.com", 100).await);
    }

    #[tokio::test]
    async fn invalidate_forca_recarga() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn Store> =
            Arc::new(LmdbStore::open_with_node_id(dir.path(), None).unwrap());
        let bl = Blocklist::new(store.clone(), None, 3600); // TTL longo

        assert!(!bl.is_blocked("late.com", 100).await); // snapshot vazio carregado
        store.add_blocked_domain("late.com").await.unwrap();
        // sem invalidar, o snapshot antigo (TTL longo) ainda não vê:
        assert!(!bl.is_blocked("late.com", 101).await);
        bl.invalidate().await;
        assert!(bl.is_blocked("late.com", 102).await); // recarregou
    }

    #[tokio::test]
    async fn recarrega_apos_ttl() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn Store> =
            Arc::new(LmdbStore::open_with_node_id(dir.path(), None).unwrap());
        let bl = Blocklist::new(store.clone(), None, 10);
        assert!(!bl.is_blocked("z.com", 100).await); // carrega vazio em t=100
        store.add_blocked_domain("z.com").await.unwrap();
        assert!(!bl.is_blocked("z.com", 105).await); // dentro do TTL: snapshot velho
        assert!(bl.is_blocked("z.com", 111).await); // t=111 > 100+10: recarrega
    }
}
