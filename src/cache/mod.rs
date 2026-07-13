use crate::store::{Record, Store, StoreError};
use moka::sync::Cache as Moka;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

pub const BREAKER_THRESHOLD: u32 = 5;
pub const BREAKER_COOLDOWN_SECS: u64 = 30;
pub const L1_TTL_SECS: u64 = 60;
pub const L2_TTL_SECS: u64 = 3600;

#[derive(Debug)]
pub struct TierError(pub String);
impl std::fmt::Display for TierError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "tier: {}", self.0)
    }
}
impl std::error::Error for TierError {}

/// Camada L2 (rede) plugável — hoje só a impl fake dos testes; o Tijolo 3
/// Task 2 adiciona `ValkeyTier` (e o `pub mod valkey;` correspondente). Erros
/// de tier nunca propagam pro chamador: o `Cache::get` os registra no
/// `Breaker` e cai pro store.
#[async_trait::async_trait]
pub trait CacheTier: Send + Sync + 'static {
    async fn get(&self, id: u64) -> Result<Option<Record>, TierError>;
    async fn set(&self, id: u64, rec: &Record, ttl_secs: u64) -> Result<(), TierError>;
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// TTL efetivo do L2 pra um registro: capado pelo TTL default e pelo tempo
/// restante até o expiry do link (não faz sentido cachear além da validade).
pub fn l2_ttl(rec: &Record, now: u64, l2_ttl_secs: u64) -> u64 {
    match rec.expiry {
        Some(e) if e > now => (e - now).min(l2_ttl_secs),
        Some(_) => 0, // já expirado (não deveria chegar aqui, mas total)
        None => l2_ttl_secs,
    }
}

/// Circuit breaker simples via atomics (sem locks): abre após
/// `BREAKER_THRESHOLD` falhas consecutivas, volta a permitir (half-open)
/// depois de `BREAKER_COOLDOWN_SECS`. Uma falha no half-open reabre.
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
    /// Deve consultar o L2 agora?
    fn allow(&self) -> bool {
        let opened = self.opened_at.load(Ordering::Relaxed);
        if opened == 0 {
            return true; // fechado
        }
        // aberto: só permite (half-open) após o cooldown
        now().saturating_sub(opened) >= BREAKER_COOLDOWN_SECS
    }
    fn record_success(&self) {
        self.failures.store(0, Ordering::Relaxed);
        self.opened_at.store(0, Ordering::Relaxed);
    }
    fn record_failure(&self) {
        let f = self.failures.fetch_add(1, Ordering::Relaxed) + 1;
        if f >= BREAKER_THRESHOLD && self.opened_at.load(Ordering::Relaxed) == 0 {
            self.opened_at.store(now(), Ordering::Relaxed);
        }
    }
}

pub struct Cache {
    store: Arc<dyn Store>,
    hot: Moka<u64, Record>,
    l2: Option<Arc<dyn CacheTier>>,
    l2_ttl_secs: u64,
    breaker: Breaker,
}

impl Cache {
    pub fn new(store: Arc<dyn Store>, capacity: u64) -> Cache {
        Cache::build(store, capacity, None, L1_TTL_SECS, L2_TTL_SECS)
    }

    pub fn with_l2(
        store: Arc<dyn Store>,
        capacity: u64,
        l2: Arc<dyn CacheTier>,
        l1_ttl_secs: u64,
        l2_ttl_secs: u64,
    ) -> Cache {
        Cache::build(store, capacity, Some(l2), l1_ttl_secs, l2_ttl_secs)
    }

    fn build(
        store: Arc<dyn Store>,
        capacity: u64,
        l2: Option<Arc<dyn CacheTier>>,
        l1_ttl: u64,
        l2_ttl: u64,
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
        }
    }

    pub async fn get(&self, id: u64) -> Result<Option<Record>, StoreError> {
        // L1
        if let Some(rec) = self.hot.get(&id) {
            return Ok(Some(rec));
        }
        // L2 (best-effort, protegido por breaker; erro nunca propaga)
        let mut l2_failed_this_request = false;
        if let Some(l2) = &self.l2 {
            if self.breaker.allow() {
                match l2.get(id).await {
                    Ok(Some(rec)) => {
                        self.breaker.record_success();
                        self.hot.insert(id, rec.clone());
                        return Ok(Some(rec));
                    }
                    Ok(None) => {
                        self.breaker.record_success();
                    }
                    Err(_) => {
                        self.breaker.record_failure();
                        l2_failed_this_request = true;
                    }
                }
            }
        }
        // store
        match self.store.get_link(id).await? {
            Some(rec) => {
                // Não tenta o L2.set se ele já falhou nesta mesma requisição
                // (evita martelar um L2 que acabamos de ver caído).
                if let Some(l2) = &self.l2 {
                    if !l2_failed_this_request && self.breaker.allow() {
                        let ttl = l2_ttl(&rec, now(), self.l2_ttl_secs);
                        if ttl > 0 {
                            match l2.set(id, &rec, ttl).await {
                                Ok(()) => self.breaker.record_success(),
                                Err(_) => self.breaker.record_failure(),
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
        }
    }

    // Tier fake que sempre falha (simula Valkey caído).
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
    }

    #[tokio::test]
    async fn sem_l2_comporta_como_hoje() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn Store> = Arc::new(LmdbStore::open(dir.path()).unwrap());
        store.put_link(3, &rec("u")).await.unwrap();
        let c = Cache::new(store, 1000);
        assert_eq!(c.get(3).await.unwrap().unwrap().url, "u");
        assert!(c.get(404).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn l2_caido_cai_no_store_e_abre_breaker() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn Store> = Arc::new(LmdbStore::open(dir.path()).unwrap());
        store.put_link(7, &rec("v")).await.unwrap();
        let tier = Arc::new(FailingTier {
            calls: AtomicU32::new(0),
        });
        let c = Cache::with_l2(store, 1000, tier.clone(), 60, 3600);
        // primeira leitura popula L1 a partir do store (e já testa o caminho
        // normal); repetimos no mesmo id só pra exercer o hit de L1.
        for _ in 0..7 {
            let _ = c.get(7).await.unwrap();
        }
        // Chama com ids DISTINTOS pra sempre furar o L1 e exercer o L2:
        for id in 100..110u64 {
            let _ = c.get(id).await.unwrap(); // store miss -> None; L2 consultado até abrir
        }
        // o breaker deve ter parado de chamar o L2 (calls não cresce indefinidamente)
        let calls = tier.calls.load(Ordering::SeqCst);
        assert!(
            calls >= BREAKER_THRESHOLD,
            "deveria ter tentado o L2 ao menos o threshold"
        );
        assert!(
            calls <= BREAKER_THRESHOLD + 2,
            "após abrir, para de chamar o L2 (calls={calls})"
        );
    }

    #[test]
    fn l2_ttl_limitado_pelo_expiry() {
        let now = 1000u64;
        // sem expiry -> usa o default
        assert_eq!(
            l2_ttl(
                &Record {
                    url: "".into(),
                    expiry: None,
                    created: 0
                },
                now,
                3600
            ),
            3600
        );
        // expiry perto -> ttl reduzido
        assert_eq!(
            l2_ttl(
                &Record {
                    url: "".into(),
                    expiry: Some(now + 100),
                    created: 0
                },
                now,
                3600
            ),
            100
        );
        // expiry longe -> cap no default
        assert_eq!(
            l2_ttl(
                &Record {
                    url: "".into(),
                    expiry: Some(now + 999_999),
                    created: 0
                },
                now,
                3600
            ),
            3600
        );
    }
}
