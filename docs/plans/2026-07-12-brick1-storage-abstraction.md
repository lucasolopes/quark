# Tijolo 1 — Abstração de storage — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Transformar o `Store` (struct concreta amarrada ao LMDB) numa trait `Store` async, com o LMDB virando uma implementação (`LmdbStore`) por trás dela, selecionável por um factory. Sem feature nova, sem mudança de comportamento observável.

**Architecture:** `src/store/mod.rs` define a trait `Store` (async, dyn-compatível via `async-trait`), `Record`, `StoreError` e `open_store()` (factory). `src/store/lmdb.rs` tem `LmdbStore` implementando a trait (código atual movido). `Cache`, `AppState` e `main` passam a usar `Arc<dyn Store>`.

**Tech Stack:** Rust 2021, async-trait, heed (LMDB), moka, axum, tokio.

## Global Constraints

- Edição Rust 2021; toolchain estável.
- Nenhum `panic!`/`unwrap()`/`expect()` no caminho de request; `expect` só no startup (`main`) e em testes.
- Comportamento observável **idêntico**: mesmas rotas, status, headers; os 29 testes existentes seguem verdes.
- Formato do LMDB inalterado (DBs `links`/`aliases`/`meta`); um `/data` existente continua válido.
- Backend selecionado em runtime via `Arc<dyn Store>`; default `lmdb`.
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` verdes (o CI exige).
- NÃO commitar `.superpowers/`, `target/`, `data/`.
- cargo NÃO está no PATH: prefixar todo comando cargo com `export PATH="$HOME/.cargo/bin:$PATH"`.

---

### Task 1: Converter `Store` em trait async + `LmdbStore` (conversão atômica)

**Files:**
- Modify: `Cargo.toml` (add `async-trait`)
- Create: `src/store/mod.rs` (era `src/store.rs`)
- Create: `src/store/lmdb.rs`
- Delete: `src/store.rs` (vira o diretório)
- Modify: `src/cache.rs`
- Modify: `src/api.rs`
- Modify: `src/main.rs`
- Test: `tests/store_it.rs` (async), `tests/api_it.rs` (helper), testes unit de `cache`/`api`

**Interfaces:**
- Consumes: heed, moka, async-trait.
- Produces:
  - `pub trait Store: Send + Sync + 'static` (async, via `#[async_trait::async_trait]`) com:
    `next_id() -> Result<u64, StoreError>`,
    `get_link(id: u64) -> Result<Option<Record>, StoreError>`,
    `put_link(id: u64, rec: &Record) -> Result<(), StoreError>`,
    `get_alias(alias: &str) -> Result<Option<u64>, StoreError>`,
    `put_alias(alias: &str, id: u64) -> Result<bool, StoreError>`,
    `put_alias_and_link(alias: &str, id: u64, rec: &Record) -> Result<bool, StoreError>`.
  - `pub struct Record { url: String, expiry: Option<u64>, created: u64 }` (inalterado).
  - `pub enum StoreError` (inalterado).
  - `pub struct LmdbStore` em `store::lmdb`, `impl Store for LmdbStore`.
  - `pub async fn open_store(path: &std::path::Path) -> Result<std::sync::Arc<dyn Store>, StoreError>`.

- [ ] **Step 1: Adicionar dependência**

Em `Cargo.toml`, `[dependencies]`, adicionar:
```toml
async-trait = "0.1"
```

- [ ] **Step 2: Criar `src/store/lmdb.rs` com o código atual movido, implementando a trait**

Mover o conteúdo do `src/store.rs` atual para cá, renomeando `Store` → `LmdbStore` e transformando os métodos em `impl Store for LmdbStore` (async). O corpo síncrono do heed roda inline dentro dos `async fn` (leitura mmap em microssegundos; sem `await` real).

```rust
use crate::store::{Record, Store, StoreError};
use heed::byteorder::BigEndian;
use heed::types::{Bytes, Str, U64};
use heed::{Database, Env, EnvOpenOptions};
use std::path::Path;

type BeU64 = U64<BigEndian>;

pub struct LmdbStore {
    env: Env,
    links: Database<BeU64, Bytes>,
    aliases: Database<Str, BeU64>,
    meta: Database<Str, BeU64>,
}

impl LmdbStore {
    pub fn open(path: &Path) -> Result<LmdbStore, StoreError> {
        std::fs::create_dir_all(path).map_err(heed::Error::Io)?;
        let env = unsafe {
            EnvOpenOptions::new()
                .map_size(64 * 1024 * 1024 * 1024)
                .max_dbs(3)
                .open(path)?
        };
        let mut wtxn = env.write_txn()?;
        let links = env.create_database(&mut wtxn, Some("links"))?;
        let aliases = env.create_database(&mut wtxn, Some("aliases"))?;
        let meta = env.create_database(&mut wtxn, Some("meta"))?;
        wtxn.commit()?;
        Ok(LmdbStore { env, links, aliases, meta })
    }
}

#[async_trait::async_trait]
impl Store for LmdbStore {
    async fn next_id(&self) -> Result<u64, StoreError> {
        let mut wtxn = self.env.write_txn()?;
        let cur = self.meta.get(&wtxn, "next_id")?.unwrap_or(0);
        let next = cur + 1;
        self.meta.put(&mut wtxn, "next_id", &next)?;
        wtxn.commit()?;
        Ok(next)
    }

    async fn get_link(&self, id: u64) -> Result<Option<Record>, StoreError> {
        let rtxn = self.env.read_txn()?;
        match self.links.get(&rtxn, &id)? {
            Some(bytes) => Ok(Some(serde_json::from_slice(bytes)?)),
            None => Ok(None),
        }
    }

    async fn put_link(&self, id: u64, rec: &Record) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec(rec)?;
        let mut wtxn = self.env.write_txn()?;
        self.links.put(&mut wtxn, &id, &bytes)?;
        wtxn.commit()?;
        Ok(())
    }

    async fn get_alias(&self, alias: &str) -> Result<Option<u64>, StoreError> {
        let rtxn = self.env.read_txn()?;
        Ok(self.aliases.get(&rtxn, alias)?)
    }

    async fn put_alias(&self, alias: &str, id: u64) -> Result<bool, StoreError> {
        let mut wtxn = self.env.write_txn()?;
        if self.aliases.get(&wtxn, alias)?.is_some() {
            return Ok(false);
        }
        self.aliases.put(&mut wtxn, alias, &id)?;
        wtxn.commit()?;
        Ok(true)
    }

    async fn put_alias_and_link(
        &self,
        alias: &str,
        id: u64,
        rec: &Record,
    ) -> Result<bool, StoreError> {
        let bytes = serde_json::to_vec(rec)?;
        let mut wtxn = self.env.write_txn()?;
        if self.aliases.get(&wtxn, alias)?.is_some() {
            return Ok(false);
        }
        self.links.put(&mut wtxn, &id, &bytes)?;
        self.aliases.put(&mut wtxn, alias, &id)?;
        wtxn.commit()?;
        Ok(true)
    }
}
```

- [ ] **Step 3: Transformar `src/store.rs` em `src/store/mod.rs` (trait + tipos + factory)**

Deletar `src/store.rs` e criar `src/store/mod.rs`:

```rust
pub mod lmdb;

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub url: String,
    pub expiry: Option<u64>,
    pub created: u64,
}

#[derive(Debug)]
pub enum StoreError {
    Db(heed::Error),
    Serde(serde_json::Error),
}
impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Db(e) => write!(f, "db: {e}"),
            StoreError::Serde(e) => write!(f, "serde: {e}"),
        }
    }
}
impl std::error::Error for StoreError {}
impl From<heed::Error> for StoreError {
    fn from(e: heed::Error) -> Self {
        StoreError::Db(e)
    }
}
impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> Self {
        StoreError::Serde(e)
    }
}

/// Interface de persistência. O caminho quente é sempre servido do cache L1;
/// os métodos async permitem backends de rede (Postgres/Valkey) nos próximos
/// tijolos sem gambiarra de bloqueio.
#[async_trait::async_trait]
pub trait Store: Send + Sync + 'static {
    async fn next_id(&self) -> Result<u64, StoreError>;
    async fn get_link(&self, id: u64) -> Result<Option<Record>, StoreError>;
    async fn put_link(&self, id: u64, rec: &Record) -> Result<(), StoreError>;
    async fn get_alias(&self, alias: &str) -> Result<Option<u64>, StoreError>;
    async fn put_alias(&self, alias: &str, id: u64) -> Result<bool, StoreError>;
    async fn put_alias_and_link(
        &self,
        alias: &str,
        id: u64,
        rec: &Record,
    ) -> Result<bool, StoreError>;
}

/// Seam de seleção de backend. Hoje só resolve LMDB; o Tijolo 4 adiciona o
/// match em `QUARK_STORE`. Async pra acomodar setup de conexão (Postgres) depois.
pub async fn open_store(path: &Path) -> Result<Arc<dyn Store>, StoreError> {
    Ok(Arc::new(lmdb::LmdbStore::open(path)?))
}
```

Nota: `src/lib.rs` já tem `pub mod store;` — com `src/store/mod.rs` no lugar de `src/store.rs`, essa linha continua válida sem alteração.

- [ ] **Step 4: Adaptar `src/cache.rs` para `Arc<dyn Store>` + `get` async**

```rust
use crate::store::{Record, Store, StoreError};
use moka::sync::Cache as Moka;
use std::sync::Arc;

pub struct Cache {
    store: Arc<dyn Store>,
    hot: Moka<u64, Record>,
}

impl Cache {
    pub fn new(store: Arc<dyn Store>, capacity: u64) -> Cache {
        Cache { store, hot: Moka::new(capacity) }
    }

    pub async fn get(&self, id: u64) -> Result<Option<Record>, StoreError> {
        if let Some(rec) = self.hot.get(&id) {
            return Ok(Some(rec));
        }
        match self.store.get_link(id).await? {
            Some(rec) => {
                self.hot.insert(id, rec.clone());
                Ok(Some(rec))
            }
            None => Ok(None),
        }
    }
}
```

Atualizar o teste in-module de `cache.rs` (`hit_e_miss`) para `#[tokio::test]` e `.await` em `cache.get(...)`; construir a store via `let store: Arc<dyn Store> = Arc::new(LmdbStore::open(dir.path()).unwrap());` (import `crate::store::lmdb::LmdbStore`).

- [ ] **Step 5: Adaptar `src/api.rs`**

- `AppState { pub cache: Cache, pub store: Arc<dyn Store>, pub key: u64 }` (o `store` agora é `Arc<dyn Store>`).
- Nos handlers, adicionar `.await` em todas as chamadas de store/cache:
  `st.store.next_id().await`, `st.store.put_link(id, &rec).await`,
  `st.store.get_alias(&code).await`, `st.store.put_alias_and_link(&alias, id, &rec).await`,
  `st.cache.get(id).await`. A lógica, os status e os headers permanecem idênticos — só os pontos de chamada ganham `await` e o tratamento de `Result` continua igual.
- Atualizar o `import` do tipo do store para `use crate::store::{Store, Record};` + `std::sync::Arc`.

- [ ] **Step 6: Adaptar `src/main.rs`**

```rust
use quark::api::{router, AppState};
use quark::cache::Cache;
use quark::store::open_store;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let path = std::env::var("QUARK_DATA").unwrap_or_else(|_| "./data".into());
    let key = std::env::var("QUARK_KEY")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or_else(|| {
            eprintln!("AVISO: QUARK_KEY não definido — usando chave de dev. NÃO use em produção.");
            0x9E3779B97F4A7C15
        });
    let store = open_store(std::path::Path::new(&path))
        .await
        .expect("abrir store");
    let cache = Cache::new(store.clone(), 100_000);
    let state = Arc::new(AppState { cache, store, key });
    let app = router(state);

    let addr = std::env::var("QUARK_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    eprintln!("quark ouvindo em {addr}");
    axum::serve(listener, app).await.expect("serve");
}
```

- [ ] **Step 7: Adaptar `tests/store_it.rs` para async + `Arc<dyn Store>`**

Trocar `use quark::store::{Record, Store};` por `use quark::store::{open_store, Record};` e usar o factory. Marcar os testes `#[tokio::test]` e `.await` nas chamadas. Ex.:

```rust
use quark::store::{open_store, Record};

fn tmp() -> tempfile::TempDir { tempfile::tempdir().unwrap() }

#[tokio::test]
async fn put_get_link() {
    let dir = tmp();
    let store = open_store(dir.path()).await.unwrap();
    let rec = Record { url: "https://example.com".into(), expiry: None, created: 100 };
    store.put_link(7, &rec).await.unwrap();
    let got = store.get_link(7).await.unwrap().unwrap();
    assert_eq!(got.url, "https://example.com");
    assert!(store.get_link(999).await.unwrap().is_none());
}
```
Adaptar `next_id_incrementa_e_persiste` (dropar o Arc pra fechar o env, reabrir via `open_store`), `alias_nao_sobrescreve` e `put_alias_and_link_atomico` do mesmo jeito (`#[tokio::test]` + `.await`). Manter as asserções idênticas.

- [ ] **Step 8: Adaptar `tests/api_it.rs` e testes unit de `api`**

No helper `app()` de `tests/api_it.rs`, trocar a construção da store: `let store = quark::store::lmdb::LmdbStore::open(dir.path()).unwrap();` e passar como `Arc<dyn Store>` (`let store: std::sync::Arc<dyn quark::store::Store> = std::sync::Arc::new(store);`), depois `Cache::new(store.clone(), 1000)` e `AppState { cache, store, key: 0x1234 }`. Os testes `#[tokio::test]` já existentes seguem iguais. Ajustar qualquer teste unit em `api.rs` que construa `AppState`/`Store` da mesma forma.

- [ ] **Step 9: fmt + clippy + build + suíte completa**

Run:
```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo build --release
cargo test
```
Expected: clippy sem warnings; build ok; **29 testes verdes** (mesmos de antes), comportamento inalterado.

- [ ] **Step 10: Smoke manual**

Run: `QUARK_DATA=./data-smoke cargo run` num terminal; noutro: `curl -s -XPOST localhost:8080/ -H 'content-type: application/json' -d '{"url":"https://example.com"}'` → JSON com `code`; `curl -si localhost:8080/<code>` → 302. Parar e `rm -rf ./data-smoke`. (Se backgroundar o server for chato no shell, confiar no `api_it` e anotar.)

- [ ] **Step 11: Commit**

```bash
git add -A
git commit -m "refactor(store): Store vira trait async + LmdbStore como backend; factory open_store (fundação plugável)"
```

---

### Task 2: Teste explícito de dyn-dispatch (prova da abstração)

**Files:**
- Create: `tests/store_trait.rs`

**Interfaces:**
- Consumes: `quark::store::{open_store, Store, Record}`.
- Produces: teste de integração provando round-trip através de `Arc<dyn Store>`.

- [ ] **Step 1: Escrever o teste**

```rust
// tests/store_trait.rs
use quark::store::{open_store, Record, Store};
use std::sync::Arc;

#[tokio::test]
async fn round_trip_via_trait_object() {
    let dir = tempfile::tempdir().unwrap();
    // Exercita explicitamente o dispatch dinâmico: nada aqui conhece o LmdbStore.
    let store: Arc<dyn Store> = open_store(dir.path()).await.unwrap();

    let id = store.next_id().await.unwrap();
    let rec = Record { url: "https://example.com/dyn".into(), expiry: None, created: 1 };
    store.put_link(id, &rec).await.unwrap();

    let got = store.get_link(id).await.unwrap().unwrap();
    assert_eq!(got.url, "https://example.com/dyn");

    // alias transacional também via trait object
    assert!(store.put_alias_and_link("promo-dyn", 999, &rec).await.unwrap());
    assert_eq!(store.get_alias("promo-dyn").await.unwrap(), Some(999));
}
```

- [ ] **Step 2: Rodar**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --test store_trait`
Expected: PASS (1 teste). Confirma que operar só via `Arc<dyn Store>` funciona (é o contrato que Postgres/Valkey vão cumprir).

- [ ] **Step 3: Suíte completa + commit**

Run: `cargo test` (tudo verde, agora 30 testes).
```bash
git add tests/store_trait.rs
git commit -m "test(store): round-trip via Arc<dyn Store> (prova do dispatch dinâmico da fundação)"
```

---

## Self-Review (feito pelo autor do plano)

**Cobertura do spec:**
- Trait `Store` async → Task 1 Step 3. ✓
- LmdbStore como impl → Task 1 Step 2. ✓
- Factory `open_store` (seam de config) → Task 1 Step 3. ✓
- Cache/AppState/main via `Arc<dyn Store>` → Task 1 Steps 4-6. ✓
- Testes adaptados async + teste via trait object → Task 1 Steps 7-8, Task 2. ✓
- `async-trait` dep → Task 1 Step 1. ✓
- Comportamento/formato LMDB inalterado → invariante checado no Step 9/10. ✓
- Cache-tier L1/L2 deferido → não está no plano (correto, é Tijolo 3). ✓

**Placeholders:** nenhum — todo step de código tem código real.

**Consistência de tipos:** `Store` (6 métodos async), `Record`, `StoreError`, `Cache::new(Arc<dyn Store>, u64)`, `open_store(&Path) -> Arc<dyn Store>`, `AppState.store: Arc<dyn Store>` usados de forma consistente entre as tasks.

**Nota de atomicidade:** a Task 1 é uma conversão atômica (o crate só compila com trait+impl+consumidores consistentes) — por isso os passos 2-8 formam um único commit no Step 11, não commits parciais. A Task 2 é puramente aditiva e compila sobre a Task 1.
