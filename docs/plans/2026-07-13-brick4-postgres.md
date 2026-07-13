# Tijolo 4 — Backend Postgres — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`).

**Goal:** `PostgresStore` (sqlx) implementando `Store` + `AnalyticsSink`, selecionável por `QUARK_DATABASE_URL`. Sem a env, default LMDB (inalterado).

**Architecture:** `src/store/postgres.rs`: `PostgresStore { pool: PgPool }` com schema idempotente no `open` (CREATE IF NOT EXISTS). `open_backends` escolhe LMDB vs Postgres por env. `StoreError` ganha `Backend(String)`. APIs runtime do sqlx (não `query!` macro) → build/CI não exigem Postgres.

**Tech Stack:** Rust 2021, sqlx 0.8 (runtime-tokio, postgres), async-trait, tokio.

## Global Constraints

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` verdes (CI).
- Testes que exigem Postgres são **gated** por `QUARK_TEST_DATABASE_URL` (skip se ausente) → CI verde sem serviço; validar contra Postgres Docker durante a implementação.
- Usar APIs **runtime** do sqlx (`sqlx::query`/`query_scalar`/`query_as` + `.bind`), NÃO as macros compile-time (`query!`) — senão o build exige um Postgres.
- ids são u64 ≤ 2^40; armazenar como `BIGINT`/`i64` (cabe em i64), cast `id as i64` / `v as u64`.
- Sem `panic!`/`unwrap()`/`expect()` no caminho de request; `expect` só em main/testes.
- Default preservado: sem `QUARK_DATABASE_URL` → LMDB, comportamento atual.
- cargo NÃO no PATH: `export PATH="$HOME/.cargo/bin:$PATH"`. Docker disponível.
- NÃO commitar `.superpowers/`, `target/`, `data/`.

---

### Task 1: `PostgresStore` — impl `Store` (sqlx, schema, sequência de id)

**Files:**
- Create: `src/store/postgres.rs`
- Modify: `src/store/mod.rs` (`StoreError::Backend`, `pub mod postgres;`)
- Modify: `Cargo.toml` (sqlx)
- Test: `tests/postgres_store_it.rs` (gated)

**Interfaces:**
- Produces: `PostgresStore { pool: sqlx::PgPool }`, `pub async fn open(url: &str) -> Result<PostgresStore, StoreError>` (cria pool + schema idempotente), `impl Store for PostgresStore`. `StoreError::Backend(String)`.

- [ ] **Step 1: Escrever o teste de integração gated**

```rust
// tests/postgres_store_it.rs
use quark::store::{postgres::PostgresStore, Record, Store};

async fn fresh() -> Option<PostgresStore> {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").ok()?;
    let s = PostgresStore::open(&url).await.unwrap();
    // limpa estado entre rodadas
    s.reset_for_tests().await.unwrap();
    Some(s)
}

#[tokio::test]
async fn put_get_link_pg() {
    let Some(s) = fresh().await else { eprintln!("skip: sem QUARK_TEST_DATABASE_URL"); return; };
    let rec = Record { url: "https://example.com".into(), expiry: None, created: 100 };
    s.put_link(7, &rec).await.unwrap();
    assert_eq!(s.get_link(7).await.unwrap().unwrap().url, "https://example.com");
    assert!(s.get_link(999).await.unwrap().is_none());
}

#[tokio::test]
async fn next_id_incrementa_pg() {
    let Some(s) = fresh().await else { return; };
    let a = s.next_id().await.unwrap();
    let b = s.next_id().await.unwrap();
    assert_eq!(b, a + 1);
}

#[tokio::test]
async fn alias_atomico_sem_orfao_pg() {
    let Some(s) = fresh().await else { return; };
    let rec = Record { url: "u".into(), expiry: None, created: 0 };
    assert!(s.put_alias_and_link("promo", 5, &rec).await.unwrap());
    assert!(!s.put_alias_and_link("promo", 9, &rec).await.unwrap()); // colisão
    assert_eq!(s.get_alias("promo").await.unwrap(), Some(5));
    assert!(s.get_link(9).await.unwrap().is_none()); // sem órfão
}
```
(`reset_for_tests` é um método `#[cfg(...)]`-agnóstico simples que dropa/trunca as tabelas — ver Step 3.)

- [ ] **Step 2: Confirmar que compila e skipa sem serviço**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --test postgres_store_it`
Expected: compila; testes fazem early-return (skip) sem a env.

- [ ] **Step 3: Implementar**

`Cargo.toml`: `sqlx = { version = "0.8", features = ["runtime-tokio", "postgres"] }`.

`src/store/mod.rs`: adicionar `pub mod postgres;` e a variante:
```rust
pub enum StoreError { Db(heed::Error), Serde(serde_json::Error), Backend(String) }
```
Atualizar o `Display` pra cobrir `Backend(s) => write!(f, "backend: {s}")`.

`src/store/postgres.rs`:
```rust
use crate::store::{Record, Store, StoreError};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};

pub struct PostgresStore { pool: PgPool }

impl PostgresStore {
    pub async fn open(url: &str) -> Result<PostgresStore, StoreError> {
        let pool = PgPoolOptions::new().max_connections(10).connect(url).await
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        let s = PostgresStore { pool };
        s.init_schema().await?;
        Ok(s)
    }

    async fn init_schema(&self) -> Result<(), StoreError> {
        for ddl in [
            "CREATE SEQUENCE IF NOT EXISTS quark_id_seq",
            "CREATE TABLE IF NOT EXISTS links (id BIGINT PRIMARY KEY, url TEXT NOT NULL, expiry BIGINT, created BIGINT NOT NULL)",
            "CREATE TABLE IF NOT EXISTS aliases (alias TEXT PRIMARY KEY, id BIGINT NOT NULL)",
            "CREATE TABLE IF NOT EXISTS stats (id BIGINT PRIMARY KEY, agg JSONB NOT NULL)",
            "CREATE TABLE IF NOT EXISTS events (id BIGINT PRIMARY KEY, recent JSONB NOT NULL)",
        ] {
            sqlx::query(ddl).execute(&self.pool).await.map_err(|e| StoreError::Backend(e.to_string()))?;
        }
        Ok(())
    }

    /// Uso em testes: zera todo o estado.
    pub async fn reset_for_tests(&self) -> Result<(), StoreError> {
        for q in ["TRUNCATE links, aliases, stats, events", "ALTER SEQUENCE quark_id_seq RESTART WITH 1"] {
            sqlx::query(q).execute(&self.pool).await.map_err(|e| StoreError::Backend(e.to_string()))?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl Store for PostgresStore {
    async fn next_id(&self) -> Result<u64, StoreError> {
        let row = sqlx::query("SELECT nextval('quark_id_seq') AS id")
            .fetch_one(&self.pool).await.map_err(|e| StoreError::Backend(e.to_string()))?;
        let id: i64 = row.try_get("id").map_err(|e| StoreError::Backend(e.to_string()))?;
        Ok(id as u64)
    }

    async fn get_link(&self, id: u64) -> Result<Option<Record>, StoreError> {
        let row = sqlx::query("SELECT url, expiry, created FROM links WHERE id = $1")
            .bind(id as i64).fetch_optional(&self.pool).await
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        match row {
            Some(r) => {
                let url: String = r.try_get("url").map_err(|e| StoreError::Backend(e.to_string()))?;
                let expiry: Option<i64> = r.try_get("expiry").map_err(|e| StoreError::Backend(e.to_string()))?;
                let created: i64 = r.try_get("created").map_err(|e| StoreError::Backend(e.to_string()))?;
                Ok(Some(Record { url, expiry: expiry.map(|v| v as u64), created: created as u64 }))
            }
            None => Ok(None),
        }
    }

    async fn put_link(&self, id: u64, rec: &Record) -> Result<(), StoreError> {
        sqlx::query("INSERT INTO links (id, url, expiry, created) VALUES ($1,$2,$3,$4) \
                     ON CONFLICT (id) DO UPDATE SET url=$2, expiry=$3, created=$4")
            .bind(id as i64).bind(&rec.url).bind(rec.expiry.map(|v| v as i64)).bind(rec.created as i64)
            .execute(&self.pool).await.map_err(|e| StoreError::Backend(e.to_string()))?;
        Ok(())
    }

    async fn get_alias(&self, alias: &str) -> Result<Option<u64>, StoreError> {
        let row = sqlx::query("SELECT id FROM aliases WHERE alias = $1")
            .bind(alias).fetch_optional(&self.pool).await
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        match row {
            Some(r) => { let id: i64 = r.try_get("id").map_err(|e| StoreError::Backend(e.to_string()))?; Ok(Some(id as u64)) }
            None => Ok(None),
        }
    }

    async fn put_alias_and_link(&self, alias: &str, id: u64, rec: &Record) -> Result<bool, StoreError> {
        let mut tx = self.pool.begin().await.map_err(|e| StoreError::Backend(e.to_string()))?;
        let res = sqlx::query("INSERT INTO aliases (alias, id) VALUES ($1,$2) ON CONFLICT (alias) DO NOTHING")
            .bind(alias).bind(id as i64).execute(&mut *tx).await.map_err(|e| StoreError::Backend(e.to_string()))?;
        if res.rows_affected() == 0 {
            // alias já existe -> rollback (drop) e false
            return Ok(false);
        }
        sqlx::query("INSERT INTO links (id, url, expiry, created) VALUES ($1,$2,$3,$4) \
                     ON CONFLICT (id) DO UPDATE SET url=$2, expiry=$3, created=$4")
            .bind(id as i64).bind(&rec.url).bind(rec.expiry.map(|v| v as i64)).bind(rec.created as i64)
            .execute(&mut *tx).await.map_err(|e| StoreError::Backend(e.to_string()))?;
        tx.commit().await.map_err(|e| StoreError::Backend(e.to_string()))?;
        Ok(true)
    }
}
```
(Confirmar a API sqlx 0.8: `PgPoolOptions`, `Row::try_get`, `execute(&mut *tx)`, `res.rows_affected()`. Ajustar se a minor diferir — ex.: import de `Row`/`Executor`.)

- [ ] **Step 4: Validar contra Postgres real (Docker)**

```bash
docker run -d --name quark-pg -p 5432:5432 -e POSTGRES_PASSWORD=postgres postgres:16
sleep 4
export PATH="$HOME/.cargo/bin:$PATH"
QUARK_TEST_DATABASE_URL="postgres://postgres:postgres@127.0.0.1:5432/postgres" cargo test --test postgres_store_it -- --nocapture
docker rm -f quark-pg
```
Expected: os 3 testes PASS contra o Postgres. (Se a rede Docker host↔container em 5432 for bloqueada, tentar `--network host`; senão rodar o teste dentro de um container na mesma rede.) `cargo test` sem a env fica verde (skip); fmt + clippy limpos.

- [ ] **Step 5: Commit**

```bash
git add src/store/postgres.rs src/store/mod.rs Cargo.toml Cargo.lock tests/postgres_store_it.rs
git commit -m "feat(store): PostgresStore impl Store (sqlx, sequência de id, alias atômico); StoreError::Backend"
```

---

### Task 2: `PostgresStore` — impl `AnalyticsSink` (stats/events JSONB)

**Files:**
- Modify: `src/store/postgres.rs`
- Test: `tests/postgres_analytics_it.rs` (gated)

**Interfaces:**
- Produces: `impl AnalyticsSink for PostgresStore` (record_batch upsert de Aggregates + append/truncate de eventos; stats).

- [ ] **Step 1: Teste de integração gated**

```rust
// tests/postgres_analytics_it.rs
use quark::analytics::{AnalyticsSink, ClickEvent};
use quark::store::postgres::PostgresStore;

async fn fresh() -> Option<PostgresStore> {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").ok()?;
    let s = PostgresStore::open(&url).await.unwrap();
    s.reset_for_tests().await.unwrap();
    Some(s)
}
fn ev(id: u64, ts: u64) -> ClickEvent { ClickEvent { id, ts, referer: None, country: Some("BR".into()), user_agent: Some("iPhone".into()) } }

#[tokio::test]
async fn record_e_stats_pg() {
    let Some(s) = fresh().await else { return; };
    s.record_batch(&[ev(1, 1_752_300_000), ev(1, 1_752_300_050)]).await.unwrap();
    let st = s.stats(1).await.unwrap().unwrap();
    assert_eq!(st.aggregates.total, 2);
    assert_eq!(st.aggregates.per_country.get("BR"), Some(&2));
    assert_eq!(st.recent.len(), 2);
    assert!(s.stats(999).await.unwrap().is_none());
}

#[tokio::test]
async fn retencao_pg() {
    let Some(s) = fresh().await else { return; };
    for b in 0..12u64 {
        let evs: Vec<ClickEvent> = (0..100).map(|i| ev(7, 1_752_300_000 + b*100 + i)).collect();
        s.record_batch(&evs).await.unwrap();
    }
    let st = s.stats(7).await.unwrap().unwrap();
    assert_eq!(st.aggregates.total, 1200);
    assert_eq!(st.recent.len(), 1000);
}
```

- [ ] **Step 2: Confirmar skip sem serviço**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --test postgres_analytics_it`
Expected: compila + skip.

- [ ] **Step 3: Implementar (em `src/store/postgres.rs`)**

```rust
use crate::analytics::{AnalyticsSink, Aggregates, ClickEvent, Stats, EVENTS_MAX};

#[async_trait::async_trait]
impl AnalyticsSink for PostgresStore {
    async fn record_batch(&self, events: &[ClickEvent]) -> Result<(), StoreError> {
        if events.is_empty() { return Ok(()); }
        use std::collections::BTreeMap;
        let mut by_id: BTreeMap<u64, Vec<&ClickEvent>> = BTreeMap::new();
        for e in events { by_id.entry(e.id).or_default().push(e); }
        let mut tx = self.pool.begin().await.map_err(|e| StoreError::Backend(e.to_string()))?;
        for (id, evs) in by_id {
            // agregados
            let row = sqlx::query("SELECT agg FROM stats WHERE id=$1").bind(id as i64)
                .fetch_optional(&mut *tx).await.map_err(|e| StoreError::Backend(e.to_string()))?;
            let mut agg: Aggregates = match row {
                Some(r) => { let v: serde_json::Value = r.try_get("agg").map_err(|e| StoreError::Backend(e.to_string()))?; serde_json::from_value(v)? }
                None => Aggregates::default(),
            };
            for e in &evs { agg.apply(e); }
            let aggv = serde_json::to_value(&agg)?;
            sqlx::query("INSERT INTO stats (id, agg) VALUES ($1,$2) ON CONFLICT (id) DO UPDATE SET agg=$2")
                .bind(id as i64).bind(&aggv).execute(&mut *tx).await.map_err(|e| StoreError::Backend(e.to_string()))?;
            // eventos crus (ring)
            let row = sqlx::query("SELECT recent FROM events WHERE id=$1").bind(id as i64)
                .fetch_optional(&mut *tx).await.map_err(|e| StoreError::Backend(e.to_string()))?;
            let mut recent: Vec<ClickEvent> = match row {
                Some(r) => { let v: serde_json::Value = r.try_get("recent").map_err(|e| StoreError::Backend(e.to_string()))?; serde_json::from_value(v)? }
                None => Vec::new(),
            };
            for e in &evs { recent.push((*e).clone()); }
            if recent.len() > EVENTS_MAX { let d = recent.len() - EVENTS_MAX; recent.drain(0..d); }
            let recv = serde_json::to_value(&recent)?;
            sqlx::query("INSERT INTO events (id, recent) VALUES ($1,$2) ON CONFLICT (id) DO UPDATE SET recent=$2")
                .bind(id as i64).bind(&recv).execute(&mut *tx).await.map_err(|e| StoreError::Backend(e.to_string()))?;
        }
        tx.commit().await.map_err(|e| StoreError::Backend(e.to_string()))?;
        Ok(())
    }

    async fn stats(&self, id: u64) -> Result<Option<Stats>, StoreError> {
        let row = sqlx::query("SELECT agg FROM stats WHERE id=$1").bind(id as i64)
            .fetch_optional(&self.pool).await.map_err(|e| StoreError::Backend(e.to_string()))?;
        let agg: Aggregates = match row {
            Some(r) => { let v: serde_json::Value = r.try_get("agg").map_err(|e| StoreError::Backend(e.to_string()))?; serde_json::from_value(v)? }
            None => return Ok(None),
        };
        let row = sqlx::query("SELECT recent FROM events WHERE id=$1").bind(id as i64)
            .fetch_optional(&self.pool).await.map_err(|e| StoreError::Backend(e.to_string()))?;
        let recent: Vec<ClickEvent> = match row {
            Some(r) => { let v: serde_json::Value = r.try_get("recent").map_err(|e| StoreError::Backend(e.to_string()))?; serde_json::from_value(v)? }
            None => Vec::new(),
        };
        Ok(Some(Stats { aggregates: agg, recent }))
    }
}
```
(Nota: `StoreError` precisa de `From<serde_json::Error>` — já existe, do Tijolo 1.)

- [ ] **Step 4: Validar contra Postgres Docker** (mesmo padrão da Task 1 Step 4, com `cargo test --test postgres_analytics_it`). PASS.

- [ ] **Step 5: Commit**

```bash
git add src/store/postgres.rs tests/postgres_analytics_it.rs
git commit -m "feat(analytics): PostgresStore impl AnalyticsSink (stats/events JSONB, retenção N)"
```

---

### Task 3: Factory (seleção por env) + main + CI Postgres

**Files:**
- Modify: `src/store/mod.rs` (`open_backends` seleciona por `QUARK_DATABASE_URL`)
- Modify: `src/main.rs`
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: `open_backends` com seleção**

Ajustar a assinatura pra receber o `data_path` (LMDB) e decidir por env:
```rust
pub async fn open_backends(data_path: &Path) -> Result<(Arc<dyn Store>, Arc<dyn AnalyticsSink>), StoreError> {
    match std::env::var("QUARK_DATABASE_URL") {
        Ok(url) => {
            let pg = Arc::new(postgres::PostgresStore::open(&url).await?);
            let store: Arc<dyn Store> = pg.clone();
            let sink: Arc<dyn AnalyticsSink> = pg;
            Ok((store, sink))
        }
        Err(_) => {
            let lmdb = Arc::new(lmdb::LmdbStore::open(data_path)?);
            let store: Arc<dyn Store> = lmdb.clone();
            let sink: Arc<dyn AnalyticsSink> = lmdb;
            Ok((store, sink))
        }
    }
}
```
(Se o Tijolo 1/2 já tinha `open_backends(path)`, é só trocar o corpo pelo match. Manter `open_store` se ainda usado, ou remover se `open_backends` o substituiu.)

- [ ] **Step 2: `src/main.rs`**

`open_backends` já é chamado; garantir que passa o path e loga o backend:
```rust
let (store, sink) = open_backends(std::path::Path::new(&path)).await.expect("abrir backends");
eprintln!("backend: {}", if std::env::var("QUARK_DATABASE_URL").is_ok() { "postgres" } else { "lmdb" });
```
(Não logar a URL do Postgres crua — credencial. Log só o nome do backend.)

- [ ] **Step 3: CI — serviço Postgres**

Em `.github/workflows/ci.yml`, adicionar ao `services` do job:
```yaml
      postgres:
        image: postgres:16
        env:
          POSTGRES_PASSWORD: postgres
        ports:
          - 5432:5432
        options: >-
          --health-cmd pg_isready --health-interval 5s --health-timeout 3s --health-retries 5
```
E no step Test, env `QUARK_TEST_DATABASE_URL: postgres://postgres:postgres@127.0.0.1:5432/postgres` (junto com o `QUARK_TEST_VALKEY_URL` do Tijolo 3).

- [ ] **Step 4: Verificar + smoke**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo build --release && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: verde/limpo (gated skipam sem env).
Smoke: subir `postgres:16` Docker, rodar `QUARK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/postgres QUARK_DATA=./data-smoke cargo run --bin quark`, confirmar log "backend: postgres", criar link, redirect 302, `GET /:code` funciona; conferir `docker exec quark-pg psql -U postgres -c 'select count(*) from links'`. Limpar.

- [ ] **Step 5: Commit**

```bash
git add src/store/mod.rs src/main.rs .github/workflows/ci.yml
git commit -m "feat(store): open_backends seleciona Postgres via QUARK_DATABASE_URL + serviço Postgres no CI"
```

---

## Self-Review (autor do plano)

- PostgresStore impl Store (sqlx, sequência, alias atômico) → Task 1. ✓
- PostgresStore impl AnalyticsSink (JSONB, retenção) → Task 2. ✓
- Seleção por QUARK_DATABASE_URL + CI → Task 3. ✓
- StoreError::Backend (erro não-heed) → Task 1. ✓
- Default LMDB preservado → Task 3 (match). ✓
- Gated tests (CI verde sem serviço) + validação Docker → Tasks 1/2/3. ✓
- APIs runtime do sqlx (não macro) → build sem Postgres → Global Constraints. ✓

**Placeholders:** nenhum. **Consistência:** `PostgresStore::{open,reset_for_tests}`, `Store`/`AnalyticsSink` impls, `open_backends(path)`, `StoreError::Backend` consistentes.
**Nota:** confirmar API sqlx 0.8 (Task 1 Step 3) contra Postgres Docker antes de assumir assinaturas.
