# Tijolo 3 — Cache L2 (Valkey) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`).

**Goal:** Cache de duas camadas no read path: L1 (moka, in-process) + L2 (Valkey, compartilhado, opt-in) com circuit breaker. Sem `QUARK_VALKEY_URL`, comportamento idêntico ao de hoje (L1+store).

**Architecture:** `src/cache.rs` vira `src/cache/mod.rs`: `Cache` (L1 moka + `Option<Arc<dyn CacheTier>>` L2 + `Breaker`). `src/cache/valkey.rs`: `ValkeyTier` (crate `redis`). Erro de L2 nunca propaga — registra no breaker e cai pro store.

**Tech Stack:** Rust 2021, moka, redis (tokio-comp), async-trait, tokio.

## Global Constraints

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` verdes (CI exige).
- **Invariante:** o redirect nunca falha/trava por causa do L2; L2 caído → breaker abre → cai no store. Testes de integração que exigem Valkey são **gated** (só rodam com `QUARK_TEST_VALKEY_URL`), pra o CI seguir verde sem serviço.
- Sem `panic!`/`unwrap()`/`expect()` no caminho de request.
- Compatibilidade: `Cache::new(store, capacity)` continua funcionando (L2=None); só adiciona construtor com L2.
- cargo NÃO no PATH: prefixar com `export PATH="$HOME/.cargo/bin:$PATH"`.
- NÃO commitar `.superpowers/`, `target/`, `data/`.
- Constantes v1: `L1_TTL_SECS=60`, `L2_TTL_SECS=3600`, `BREAKER_THRESHOLD=5`, `BREAKER_COOLDOWN_SECS=30`.

---

### Task 1: `Cache` L1+L2 com `CacheTier` trait + circuit breaker (lógica pura, testável sem serviço)

**Files:**
- Create: `src/cache/mod.rs` (era `src/cache.rs`)
- Delete: `src/cache.rs`
- Test: dentro de `src/cache/mod.rs`

**Interfaces:**
- Produces:
  - `#[async_trait::async_trait] pub trait CacheTier: Send + Sync + 'static { async fn get(&self, id: u64) -> Result<Option<Record>, TierError>; async fn set(&self, id: u64, rec: &Record, ttl_secs: u64) -> Result<(), TierError>; }`
  - `pub struct TierError(pub String);` (+ Display/Error) — erro genérico do tier (o ValkeyTier converte o `redis::RedisError`).
  - `pub struct Cache { ... }` com:
    - `pub fn new(store: Arc<dyn Store>, capacity: u64) -> Cache` (L2 None, L1 TTL default).
    - `pub fn with_l2(store: Arc<dyn Store>, capacity: u64, l2: Arc<dyn CacheTier>, l1_ttl_secs: u64, l2_ttl_secs: u64) -> Cache`.
    - `pub async fn get(&self, id: u64) -> Result<Option<Record>, StoreError>` (L1→L2→store, best-effort L2).
  - `l2_ttl(rec, now, l2_ttl_secs) -> u64` (limitado pelo expiry).
  - `Breaker` interno (atomics: threshold/cooldown).

- [ ] **Step 1: Escrever os testes que falham**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{lmdb::LmdbStore, Record, Store};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    fn rec(url: &str) -> Record { Record { url: url.into(), expiry: None, created: 0 } }

    fn store_with(id: u64, url: &str) -> (tempfile::TempDir, Arc<dyn Store>) {
        let dir = tempfile::tempdir().unwrap();
        let s: Arc<dyn Store> = Arc::new(LmdbStore::open(dir.path()).unwrap());
        (dir, s)
    }

    // Tier fake que sempre falha (simula Valkey caído).
    struct FailingTier { calls: AtomicU32 }
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
        let tier = Arc::new(FailingTier { calls: AtomicU32::new(0) });
        let c = Cache::with_l2(store, 1000, tier.clone(), 60, 3600);
        // várias leituras (cada uma miss no L1 pois capacity ok mas ids diferentes forçam? usar ids distintos)
        for id in [7u64, 7, 7, 7, 7, 7, 7] {
            // primeira popula L1; pra forçar consulta ao L2, limpamos? Em vez disso, use ids inexistentes
            let _ = c.get(id).await.unwrap();
        }
        // Após BREAKER_THRESHOLD falhas o breaker abre e para de chamar o L2.
        // Chama com ids DISTINTOS pra sempre furar o L1 e exercer o L2:
        for id in 100..110u64 {
            let _ = c.get(id).await.unwrap(); // store miss -> None; L2 consultado até abrir
        }
        // o breaker deve ter parado de chamar o L2 (calls não cresce indefinidamente)
        let calls = tier.calls.load(Ordering::SeqCst);
        assert!(calls >= BREAKER_THRESHOLD, "deveria ter tentado o L2 ao menos o threshold");
        assert!(calls <= BREAKER_THRESHOLD + 2, "após abrir, para de chamar o L2 (calls={calls})");
    }

    #[test]
    fn l2_ttl_limitado_pelo_expiry() {
        let now = 1000u64;
        // sem expiry -> usa o default
        assert_eq!(l2_ttl(&Record { url: "".into(), expiry: None, created: 0 }, now, 3600), 3600);
        // expiry perto -> ttl reduzido
        assert_eq!(l2_ttl(&Record { url: "".into(), expiry: Some(now + 100), created: 0 }, now, 3600), 100);
        // expiry longe -> cap no default
        assert_eq!(l2_ttl(&Record { url: "".into(), expiry: Some(now + 999_999), created: 0 }, now, 3600), 3600);
    }
}
```

- [ ] **Step 2: Rodar e confirmar falha**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test cache`
Expected: FAIL (símbolos não existem).

- [ ] **Step 3: Implementar `src/cache/mod.rs`**

Mover o `src/cache.rs` atual pra `src/cache/mod.rs` e reescrever:

```rust
pub mod valkey; // criado na Task 2 (por ora, arquivo vazio com `// Task 2` — ou adicionar na Task 2)

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
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "tier: {}", self.0) }
}
impl std::error::Error for TierError {}

#[async_trait::async_trait]
pub trait CacheTier: Send + Sync + 'static {
    async fn get(&self, id: u64) -> Result<Option<Record>, TierError>;
    async fn set(&self, id: u64, rec: &Record, ttl_secs: u64) -> Result<(), TierError>;
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn l2_ttl(rec: &Record, now: u64, l2_ttl_secs: u64) -> u64 {
    match rec.expiry {
        Some(e) if e > now => (e - now).min(l2_ttl_secs),
        Some(_) => 0, // já expirado (não deveria chegar aqui, mas total)
        None => l2_ttl_secs,
    }
}

struct Breaker { failures: AtomicU32, opened_at: AtomicU64 }
impl Breaker {
    fn new() -> Breaker { Breaker { failures: AtomicU32::new(0), opened_at: AtomicU64::new(0) } }
    /// Deve consultar o L2 agora?
    fn allow(&self) -> bool {
        let opened = self.opened_at.load(Ordering::Relaxed);
        if opened == 0 { return true; } // fechado
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
    fn build(store: Arc<dyn Store>, capacity: u64, l2: Option<Arc<dyn CacheTier>>, l1_ttl: u64, l2_ttl: u64) -> Cache {
        let hot = Moka::builder()
            .max_capacity(capacity)
            .time_to_live(std::time::Duration::from_secs(l1_ttl))
            .build();
        Cache { store, hot, l2, l2_ttl_secs: l2_ttl, breaker: Breaker::new() }
    }

    pub async fn get(&self, id: u64) -> Result<Option<Record>, StoreError> {
        // L1
        if let Some(rec) = self.hot.get(&id) {
            return Ok(Some(rec));
        }
        // L2 (best-effort, protegido por breaker)
        if let Some(l2) = &self.l2 {
            if self.breaker.allow() {
                match l2.get(id).await {
                    Ok(Some(rec)) => {
                        self.breaker.record_success();
                        self.hot.insert(id, rec.clone());
                        return Ok(Some(rec));
                    }
                    Ok(None) => { self.breaker.record_success(); }
                    Err(_) => { self.breaker.record_failure(); }
                }
            }
        }
        // store
        match self.store.get_link(id).await? {
            Some(rec) => {
                if let Some(l2) = &self.l2 {
                    if self.breaker.allow() {
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
```
Ajustar `src/lib.rs`: `pub mod cache;` já existe; com `src/cache/mod.rs` no lugar de `src/cache.rs`, segue válido. Nota: o teste `l2_caido...` usa ids distintos (100..110) pra furar o L1 e exercer o L2 até o breaker abrir.

- [ ] **Step 4: Rodar e confirmar passa**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test cache`
Expected: PASS. `cargo test` completo verde; `cargo fmt` + `clippy -D warnings` limpos.

- [ ] **Step 5: Commit**

```bash
git add src/cache/ src/lib.rs
git commit -m "feat(cache): L1+L2 com CacheTier trait + circuit breaker (L2 opt-in; erro de L2 cai no store)"
```

---

### Task 2: `ValkeyTier` (crate redis) — implementação do L2

**Files:**
- Create: `src/cache/valkey.rs`
- Modify: `Cargo.toml` (redis dep)
- Test: `tests/valkey_tier_it.rs` (gated)

**Interfaces:**
- Consumes: `crate::cache::{CacheTier, TierError}`, `crate::store::Record`, crate `redis`.
- Produces: `pub struct ValkeyTier { conn: redis::aio::MultiplexedConnection }` + `pub async fn open(url: &str) -> Result<ValkeyTier, TierError>` + `impl CacheTier`.

- [ ] **Step 1: Escrever o teste de integração gated**

```rust
// tests/valkey_tier_it.rs
use quark::cache::{CacheTier};
use quark::cache::valkey::ValkeyTier;
use quark::store::Record;

// Só roda se QUARK_TEST_VALKEY_URL estiver setado (ex.: redis://127.0.0.1:6379).
#[tokio::test]
async fn set_get_round_trip() {
    let Ok(url) = std::env::var("QUARK_TEST_VALKEY_URL") else {
        eprintln!("skip: QUARK_TEST_VALKEY_URL não setado");
        return;
    };
    let tier = ValkeyTier::open(&url).await.unwrap();
    let id = 424242u64;
    assert!(tier.get(id).await.unwrap().is_none() || true); // pode ter lixo de rodada anterior
    let rec = Record { url: "https://example.com/valkey".into(), expiry: None, created: 1 };
    tier.set(id, &rec, 60).await.unwrap();
    let got = tier.get(id).await.unwrap().unwrap();
    assert_eq!(got.url, "https://example.com/valkey");
}
```

- [ ] **Step 2: Confirmar que compila e o teste passa/skipa**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --test valkey_tier_it`
Expected (sem serviço): compila e o teste faz early-return (skip). Depois validar com serviço no Step 4.

- [ ] **Step 3: Implementar**

`Cargo.toml`: `redis = { version = "0.27", features = ["tokio-comp"] }`.

`src/cache/valkey.rs`:
```rust
use crate::cache::{CacheTier, TierError};
use crate::store::Record;
use redis::AsyncCommands;

pub struct ValkeyTier {
    conn: redis::aio::MultiplexedConnection,
}

impl ValkeyTier {
    pub async fn open(url: &str) -> Result<ValkeyTier, TierError> {
        let client = redis::Client::open(url).map_err(|e| TierError(e.to_string()))?;
        let conn = client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| TierError(e.to_string()))?;
        Ok(ValkeyTier { conn })
    }
    fn key(id: u64) -> String { format!("q:{id}") }
}

#[async_trait::async_trait]
impl CacheTier for ValkeyTier {
    async fn get(&self, id: u64) -> Result<Option<Record>, TierError> {
        let mut conn = self.conn.clone();
        let bytes: Option<Vec<u8>> = conn.get(Self::key(id)).await.map_err(|e| TierError(e.to_string()))?;
        match bytes {
            Some(b) => serde_json::from_slice(&b).map(Some).map_err(|e| TierError(e.to_string())),
            None => Ok(None),
        }
    }
    async fn set(&self, id: u64, rec: &Record, ttl_secs: u64) -> Result<(), TierError> {
        let mut conn = self.conn.clone();
        let b = serde_json::to_vec(rec).map_err(|e| TierError(e.to_string()))?;
        conn.set_ex::<_, _, ()>(Self::key(id), b, ttl_secs).await.map_err(|e| TierError(e.to_string()))?;
        Ok(())
    }
}
```
(Confirmar a API exata do redis 0.27 — `get_multiplexed_async_connection`, `set_ex` assinatura/genéricos — via `~/.cargo/registry` ou docs.rs; ajustar se a minor diferir.)

- [ ] **Step 4: Validar contra um Valkey real (Docker)**

Run:
```bash
docker run -d --name quark-valkey -p 6379:6379 valkey/valkey:8
export PATH="$HOME/.cargo/bin:$PATH"
QUARK_TEST_VALKEY_URL=redis://127.0.0.1:6379 cargo test --test valkey_tier_it -- --nocapture
docker rm -f quark-valkey
```
Expected: `set_get_round_trip` PASS contra o Valkey. `cargo test` completo (sem a env) verde; fmt + clippy limpos.

- [ ] **Step 5: Commit**

```bash
git add src/cache/valkey.rs Cargo.toml Cargo.lock tests/valkey_tier_it.rs
git commit -m "feat(cache): ValkeyTier (crate redis) — impl CacheTier; teste de integração gated"
```

---

### Task 3: Wiring (main monta L2 se QUARK_VALKEY_URL) + CI service

**Files:**
- Modify: `src/main.rs`
- Modify: `.github/workflows/ci.yml`
- Test: `tests/l2_resilience_it.rs` (opcional; a resiliência já é testada na Task 1 via fake tier)

**Interfaces:**
- Consumes: `cache::{Cache, valkey::ValkeyTier}`.

- [ ] **Step 1: `src/main.rs` — montar o L2 se configurado**

```rust
use quark::cache::valkey::ValkeyTier;
// ... após open_backends e antes de construir o Cache:
let cache = match std::env::var("QUARK_VALKEY_URL").ok() {
    Some(url) => match ValkeyTier::open(&url).await {
        Ok(tier) => {
            eprintln!("L2 Valkey habilitado: {url}");
            Cache::with_l2(store.clone(), 100_000, std::sync::Arc::new(tier),
                quark::cache::L1_TTL_SECS, quark::cache::L2_TTL_SECS)
        }
        Err(e) => {
            eprintln!("AVISO: falha ao conectar no Valkey ({e}); seguindo só com L1+store.");
            Cache::new(store.clone(), 100_000)
        }
    },
    None => Cache::new(store.clone(), 100_000),
};
```
(Se o Valkey estiver configurado mas fora no boot, o app sobe com L1+store — não trava o startup.)

- [ ] **Step 2: CI — serviço Valkey + rodar os testes de integração**

Em `.github/workflows/ci.yml`, no job `check`, adicionar:
```yaml
    services:
      valkey:
        image: valkey/valkey:8
        ports:
          - 6379:6379
        options: >-
          --health-cmd "valkey-cli ping" --health-interval 5s --health-timeout 3s --health-retries 5
```
E no step de teste, exportar a env pros gated:
```yaml
      - name: Test
        run: cargo test
        env:
          QUARK_TEST_VALKEY_URL: redis://127.0.0.1:6379
```

- [ ] **Step 3: Verificar**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo build --release && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: tudo verde/limpo. (O `cargo test` local sem a env skipa os gated; a validação com Valkey real foi feita na Task 2 Step 4.)

- [ ] **Step 4: Smoke com Valkey (Docker)**

Subir `valkey/valkey:8`, rodar `QUARK_VALKEY_URL=redis://127.0.0.1:6379 QUARK_DATA=./data-smoke cargo run`; criar link, acessar 2x, confirmar no log "L2 Valkey habilitado" e que funciona; conferir a chave no Valkey (`docker exec quark-valkey valkey-cli keys 'q:*'`). Limpar.

- [ ] **Step 5: Commit**

```bash
git add src/main.rs .github/workflows/ci.yml
git commit -m "feat(cache): wiring do L2 Valkey no startup (QUARK_VALKEY_URL) + serviço Valkey no CI"
```

---

## Self-Review (autor do plano)

- CacheTier trait + breaker + L1L2 → Task 1. ✓
- ValkeyTier (redis) + gated integ test → Task 2. ✓
- Opt-in via QUARK_VALKEY_URL + CI service → Task 3. ✓
- Invariante (L2 caído → store, sem travar) → Task 1 (`l2_caido_cai_no_store_e_abre_breaker`) + startup resiliente (Task 3 Step 1). ✓
- Compat `Cache::new(store, capacity)` preservada → Task 1. ✓
- Testes de serviço gated (CI verde sem serviço) → Task 2/3. ✓

**Placeholders:** nenhum. **Consistência:** `CacheTier`/`TierError`/`Cache::{new,with_l2,get}`/`l2_ttl`/`ValkeyTier::open` usados consistentemente.
**Nota:** confirmar a API do `redis` 0.27 (Task 2 Step 3) antes de assumir assinaturas.
