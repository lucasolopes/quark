# Tijolo 5 — Sink ClickHouse — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`).

**Goal:** `ClickHouseSink` (crate `clickhouse`) implementando `AnalyticsSink` no modelo OLAP (INSERT append + agregação por query), opt-in por `QUARK_CLICKHOUSE_URL`, com o factory escolhendo Store e Sink INDEPENDENTEMENTE.

**Architecture:** `src/analytics/clickhouse.rs`: `ClickHouseSink { client }`. `record_batch` = bulk INSERT em `clicks`; `stats` = queries GROUP BY reconstruindo `Stats`. `open_backends` desacopla Store (`QUARK_DATABASE_URL`) de Sink (`QUARK_CLICKHOUSE_URL`).

**Tech Stack:** Rust 2021, clickhouse 0.13, async-trait, tokio.

## Global Constraints

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` verdes (CI).
- Testes que exigem ClickHouse são **gated** por `QUARK_TEST_CLICKHOUSE_URL` (skip se ausente) → CI verde sem serviço; validar contra ClickHouse Docker na implementação.
- `record_batch` roda no worker (erro logado, não afeta redirect); `stats` só no endpoint admin (erro → 503). Sem panic no request path.
- ids u64 ≤ 2^40; ts epoch secs; country/referer '' quando ausente.
- Default preservado: sem `QUARK_CLICKHOUSE_URL`, o sink é o do Store (LMDB/Postgres) — comportamento atual.
- cargo NÃO no PATH: `export PATH="$HOME/.cargo/bin:$PATH"`. Docker disponível.
- NÃO commitar `.superpowers/`, `target/`, `data/`.

---

### Task 1: `ClickHouseSink` — impl `AnalyticsSink` (append + agregação por query)

**Files:**
- Create: `src/analytics/clickhouse.rs` (converter `src/analytics.rs` → `src/analytics/mod.rs` + `clickhouse.rs`)
- Modify: `src/analytics/mod.rs` (add `pub mod clickhouse;`)
- Modify: `Cargo.toml` (clickhouse)
- Test: `tests/clickhouse_sink_it.rs` (gated)

**Interfaces:**
- Produces: `ClickHouseSink { client: clickhouse::Client }`, `pub async fn open(url: &str) -> Result<ClickHouseSink, StoreError>` (client + `CREATE TABLE IF NOT EXISTS clicks`), `impl AnalyticsSink`.

- [ ] **Step 1: Teste de integração gated**

```rust
// tests/clickhouse_sink_it.rs
use quark::analytics::{AnalyticsSink, ClickEvent};
use quark::analytics::clickhouse::ClickHouseSink;

async fn fresh() -> Option<ClickHouseSink> {
    let url = std::env::var("QUARK_TEST_CLICKHOUSE_URL").ok()?;
    let s = ClickHouseSink::open(&url).await.unwrap();
    s.reset_for_tests().await.unwrap();
    Some(s)
}
fn ev(id: u64, ts: u64, c: &str, ua: &str) -> ClickEvent {
    ClickEvent { id, ts, referer: None, country: Some(c.into()), user_agent: Some(ua.into()) }
}

#[tokio::test]
async fn record_e_stats_ch() {
    let Some(s) = fresh().await else { eprintln!("skip: sem QUARK_TEST_CLICKHOUSE_URL"); return; };
    s.record_batch(&[
        ev(1, 1_752_300_000, "BR", "iPhone"),
        ev(1, 1_752_300_050, "BR", "Windows NT 10.0"),
        ev(1, 1_752_400_000, "US", "curl/8"),
    ]).await.unwrap();
    let st = s.stats(1).await.unwrap().unwrap();
    assert_eq!(st.aggregates.total, 3);
    assert_eq!(st.aggregates.per_country.get("BR"), Some(&2));
    assert_eq!(st.aggregates.per_country.get("US"), Some(&1));
    assert_eq!(st.aggregates.per_device.get("Mobile"), Some(&1));
    assert_eq!(st.aggregates.per_device.get("Desktop"), Some(&1));
    assert_eq!(st.aggregates.per_device.get("Other"), Some(&1));
    assert_eq!(st.recent.len(), 3);
    assert!(s.stats(999).await.unwrap().is_none());
}

#[tokio::test]
async fn recent_limita_n_ch() {
    let Some(s) = fresh().await else { return; };
    let evs: Vec<ClickEvent> = (0..1200u64).map(|i| ev(7, 1_752_300_000 + i, "BR", "iPhone")).collect();
    s.record_batch(&evs).await.unwrap();
    let st = s.stats(7).await.unwrap().unwrap();
    assert_eq!(st.aggregates.total, 1200);
    assert_eq!(st.recent.len(), 1000); // últimos N
}
```

- [ ] **Step 2: Confirmar skip sem serviço**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --test clickhouse_sink_it`
Expected: compila + skip.

- [ ] **Step 3: Implementar**

`Cargo.toml`: `clickhouse = "0.13"`.

Converter `src/analytics.rs` → `src/analytics/mod.rs` (conteúdo atual intacto) + adicionar `pub mod clickhouse;`.

`src/analytics/clickhouse.rs`:
```rust
use crate::analytics::{AnalyticsSink, Aggregates, ClickEvent, Stats, device_from_ua, day_bucket, EVENTS_MAX};
use crate::store::StoreError;
use clickhouse::Row;
use serde::{Deserialize, Serialize};

#[derive(Row, Serialize)]
struct ClickRow<'a> { id: u64, ts: u64, country: &'a str, device: &'a str, referer: &'a str }

#[derive(Row, Deserialize)]
struct Totals { total: u64, first_ts: u64, last_ts: u64 }
#[derive(Row, Deserialize)]
struct Kv { k: String, c: u64 }
#[derive(Row, Deserialize)]
struct RecentRow { ts: u64, country: String, device: String, referer: String }

pub struct ClickHouseSink { client: clickhouse::Client }

impl ClickHouseSink {
    pub async fn open(url: &str) -> Result<ClickHouseSink, StoreError> {
        let client = clickhouse::Client::default().with_url(url);
        let s = ClickHouseSink { client };
        s.init_schema().await?;
        Ok(s)
    }
    async fn init_schema(&self) -> Result<(), StoreError> {
        self.client.query(
            "CREATE TABLE IF NOT EXISTS clicks (id UInt64, ts UInt64, country String, device String, referer String) ENGINE = MergeTree ORDER BY (id, ts)"
        ).execute().await.map_err(|e| StoreError::Backend(e.to_string()))
    }
    pub async fn reset_for_tests(&self) -> Result<(), StoreError> {
        self.client.query("TRUNCATE TABLE IF EXISTS clicks").execute().await
            .map_err(|e| StoreError::Backend(e.to_string()))
    }
}

#[async_trait::async_trait]
impl AnalyticsSink for ClickHouseSink {
    async fn record_batch(&self, events: &[ClickEvent]) -> Result<(), StoreError> {
        if events.is_empty() { return Ok(()); }
        let mut insert = self.client.insert("clicks").map_err(|e| StoreError::Backend(e.to_string()))?;
        for e in events {
            let device = device_from_ua(e.user_agent.as_deref());
            let row = ClickRow {
                id: e.id, ts: e.ts,
                country: e.country.as_deref().unwrap_or(""),
                device,
                referer: e.referer.as_deref().unwrap_or(""),
            };
            insert.write(&row).await.map_err(|e| StoreError::Backend(e.to_string()))?;
        }
        insert.end().await.map_err(|e| StoreError::Backend(e.to_string()))?;
        Ok(())
    }

    async fn stats(&self, id: u64) -> Result<Option<Stats>, StoreError> {
        let totals: Totals = self.client
            .query("SELECT count() AS total, min(ts) AS first_ts, max(ts) AS last_ts FROM clicks WHERE id = ?")
            .bind(id).fetch_one().await.map_err(|e| StoreError::Backend(e.to_string()))?;
        if totals.total == 0 { return Ok(None); }

        let mut agg = Aggregates { total: totals.total, first_ts: totals.first_ts, last_ts: totals.last_ts, ..Default::default() };

        let per_day: Vec<Kv> = self.client
            .query("SELECT formatDateTime(toDateTime(ts,'UTC'),'%F') AS k, count() AS c FROM clicks WHERE id = ? GROUP BY k")
            .bind(id).fetch_all().await.map_err(|e| StoreError::Backend(e.to_string()))?;
        for kv in per_day { agg.per_day.insert(kv.k, kv.c); }

        let per_country: Vec<Kv> = self.client
            .query("SELECT country AS k, count() AS c FROM clicks WHERE id = ? GROUP BY k")
            .bind(id).fetch_all().await.map_err(|e| StoreError::Backend(e.to_string()))?;
        for kv in per_country { if !kv.k.is_empty() { agg.per_country.insert(kv.k, kv.c); } }

        let per_device: Vec<Kv> = self.client
            .query("SELECT device AS k, count() AS c FROM clicks WHERE id = ? GROUP BY k")
            .bind(id).fetch_all().await.map_err(|e| StoreError::Backend(e.to_string()))?;
        for kv in per_device { agg.per_device.insert(kv.k, kv.c); }

        let mut recent_rows: Vec<RecentRow> = self.client
            .query("SELECT ts, country, device, referer FROM clicks WHERE id = ? ORDER BY ts DESC LIMIT ?")
            .bind(id).bind(EVENTS_MAX as u64).fetch_all().await.map_err(|e| StoreError::Backend(e.to_string()))?;
        recent_rows.reverse(); // cronológico
        let recent = recent_rows.into_iter().map(|r| ClickEvent {
            id, ts: r.ts,
            referer: if r.referer.is_empty() { None } else { Some(r.referer) },
            country: if r.country.is_empty() { None } else { Some(r.country) },
            user_agent: None, // ClickHouse guarda device, não o UA cru (fidelidade documentada)
        }).collect();

        Ok(Some(Stats { aggregates: agg, recent }))
    }
}
```
(Confirmar a API `clickhouse` 0.13 contra o Docker: `Client::default().with_url`, `insert(...)?.write(&row).await?.end()`, `query(...).bind(...).fetch_one::<T>()/fetch_all::<T>()`, `#[derive(Row)]`. Ajustar imports/assinaturas se a minor diferir. `day_bucket` import pode não ser usado — remover se clippy reclamar.)

- [ ] **Step 4: Validar contra ClickHouse real (Docker)**

```bash
docker run -d --name quark-ch -p 8123:8123 clickhouse/clickhouse-server:24
sleep 8
export PATH="$HOME/.cargo/bin:$PATH"
QUARK_TEST_CLICKHOUSE_URL="http://127.0.0.1:8123" cargo test --test clickhouse_sink_it -- --nocapture
docker rm -f quark-ch
```
Expected: `record_e_stats_ch` e `recent_limita_n_ch` PASS. (Se a rede host↔container em 8123 bloquear, `--network host` ou rodar dentro de um container.) Sempre `docker rm -f quark-ch`. `cargo test` sem env fica verde; fmt + clippy limpos.

- [ ] **Step 5: Commit**

```bash
git add src/analytics/ Cargo.toml Cargo.lock tests/clickhouse_sink_it.rs
git commit -m "feat(analytics): ClickHouseSink impl AnalyticsSink (append + agregação por query, opt-in)"
```

---

### Task 2: Factory desacoplado (Store vs Sink) + main + CI ClickHouse

**Files:**
- Modify: `src/store/mod.rs` (`open_backends` escolhe Sink independente)
- Modify: `src/main.rs`
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: `open_backends` desacopla Store e Sink**

```rust
pub async fn open_backends(data_path: &Path) -> Result<(Arc<dyn Store>, Arc<dyn AnalyticsSink>), StoreError> {
    // Store (+ sink embutido do próprio backend), como no Tijolo 4:
    let (store, embedded_sink): (Arc<dyn Store>, Arc<dyn AnalyticsSink>) = match std::env::var("QUARK_DATABASE_URL") {
        Ok(url) => { let pg = Arc::new(postgres::PostgresStore::open(&url).await?); (pg.clone(), pg) }
        Err(_)  => { let l = Arc::new(lmdb::LmdbStore::open(data_path)?); (l.clone(), l) }
    };
    // Sink: ClickHouse se configurado, senão o embutido:
    let sink: Arc<dyn AnalyticsSink> = match std::env::var("QUARK_CLICKHOUSE_URL") {
        Ok(url) => Arc::new(crate::analytics::clickhouse::ClickHouseSink::open(&url).await?),
        Err(_)  => embedded_sink,
    };
    Ok((store, sink))
}
```
(Reaproveita o corpo do Tijolo 4 pro Store; só adiciona a escolha do Sink.)

- [ ] **Step 2: `src/main.rs` — log do sink**

Após `open_backends`, logar o sink escolhido (sem URL):
```rust
eprintln!("analytics sink: {}", if std::env::var("QUARK_CLICKHOUSE_URL").is_ok() { "clickhouse" }
    else if std::env::var("QUARK_DATABASE_URL").is_ok() { "postgres" } else { "lmdb(embutido)" });
```
Manter o resto (backend log, worker, AppState, serve) intacto.

- [ ] **Step 3: CI — serviço ClickHouse**

Em `.github/workflows/ci.yml`, adicionar ao `services` do job (junto de valkey/postgres):
```yaml
      clickhouse:
        image: clickhouse/clickhouse-server:24
        ports:
          - 8123:8123
        options: >-
          --health-cmd "wget -q -O - http://localhost:8123/ping || exit 1" --health-interval 5s --health-timeout 3s --health-retries 10
```
E no step Test, env `QUARK_TEST_CLICKHOUSE_URL: http://127.0.0.1:8123` (junto das outras QUARK_TEST_*).

- [ ] **Step 4: Verificar + smoke**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo build --release && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: verde/limpo (gated skipam sem env).
Smoke (Docker ClickHouse): subir `clickhouse/clickhouse-server:24`, rodar `QUARK_CLICKHOUSE_URL=http://127.0.0.1:8123 QUARK_ADMIN_TOKEN=segredo QUARK_DATA=./data-smoke QUARK_ADDR=127.0.0.1:8902 cargo run --bin quark`, confirmar log "analytics sink: clickhouse", criar link, acessar 2x, esperar o flush (~6s), `curl -H 'x-admin-token: segredo' localhost:8902/<code>/stats` → JSON com total ≥ 1. Limpar (parar server, `docker rm -f quark-ch`, `rm -rf data-smoke`).

- [ ] **Step 5: Commit**

```bash
git add src/store/mod.rs src/main.rs .github/workflows/ci.yml
git commit -m "feat(analytics): open_backends escolhe sink ClickHouse via QUARK_CLICKHOUSE_URL (desacoplado do Store) + CI"
```

---

## Self-Review (autor do plano)

- ClickHouseSink impl AnalyticsSink (append + query) → Task 1. ✓
- Factory desacopla Store vs Sink → Task 2. ✓
- Opt-in QUARK_CLICKHOUSE_URL + CI → Task 2. ✓
- Default preservado (sem env → sink do Store) → Task 2. ✓
- record_batch no worker / stats no admin → sem panic no redirect → Global Constraints. ✓
- Gated + Docker validation → Task 1. ✓
- Fidelidade do recent (user_agent None) documentada → spec §10 + comentário no código. ✓

**Placeholders:** nenhum. **Consistência:** `ClickHouseSink::{open,reset_for_tests}`, `AnalyticsSink` impl, `open_backends` desacoplado.
**Nota:** confirmar API clickhouse 0.13 (Task 1 Step 3) contra Docker antes de assumir. Se `day_bucket` não for usado no clickhouse.rs, não importar.
