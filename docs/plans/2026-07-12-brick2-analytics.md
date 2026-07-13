# Tijolo 2 — Pipeline de analytics — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Analytics rico (contagem, série por dia, país, device, eventos crus) com impacto ZERO no redirect: captura fire-and-forget → worker de fundo → sink plugável (`AnalyticsSink`), com implementação embutida LMDB. Endpoint `GET /:code/stats` protegido por token.

**Architecture:** `src/analytics.rs` define `ClickEvent`, `Aggregates`/`Stats`, o trait `AnalyticsSink`, as funções puras de agregação/UA e o worker. `LmdbStore` passa a implementar também `AnalyticsSink` (mesmo env, DBs `stats`/`events`, `max_dbs=5`). O redirect faz `try_send` num `mpsc` limitado (drop-on-full); o worker agrega e grava em lote.

**Tech Stack:** Rust 2021, tokio (mpsc, spawn), async-trait, heed (LMDB), serde, axum.

## Global Constraints

- Edição Rust 2021; `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` verdes (CI exige).
- **Invariante sagrado:** o `GET /:code` responde 302 idêntico ao de hoje; no hit, no máximo um `try_send` O(1) — sem `await` de I/O, sem lock, sem cálculo. Fila cheia → descarta o evento, redirect segue.
- Nenhum `panic!`/`unwrap()`/`expect()` no caminho de request; `expect` só em main/testes.
- Analytics é **best-effort**: nunca propaga erro pro redirect.
- Coleta roda sempre; a **leitura** (`/stats`) exige `QUARK_ADMIN_TOKEN` (sem token no processo → endpoint 404).
- Formato dos DBs existentes (`links`/`aliases`/`meta`) inalterado; `/data` existente continua válido.
- cargo NÃO está no PATH: prefixar todo comando cargo com `export PATH="$HOME/.cargo/bin:$PATH"`.
- NÃO commitar `.superpowers/`, `target/`, `data/`.
- Constantes v1: `EVENTS_MAX = 1000`, `BATCH = 500`, flush interval 5s, canal capacidade 10_000.

---

### Task 1: Núcleo puro do analytics (tipos, agregação, UA, trait)

**Files:**
- Create: `src/analytics.rs`
- Modify: `src/lib.rs` (add `pub mod analytics;`)

**Interfaces:**
- Consumes: `crate::store::StoreError`.
- Produces:
  - `pub struct ClickEvent { pub id: u64, pub ts: u64, pub referer: Option<String>, pub country: Option<String>, pub user_agent: Option<String> }` (derive Clone, Serialize, Deserialize).
  - `pub enum Device { Mobile, Desktop, Other }` → representado como `&'static str` nos agregados.
  - `pub struct Aggregates { pub total: u64, pub first_ts: u64, pub last_ts: u64, pub per_day: BTreeMap<String,u64>, pub per_country: BTreeMap<String,u64>, pub per_device: BTreeMap<String,u64> }` (derive Default, Clone, Serialize, Deserialize) com `pub fn apply(&mut self, ev: &ClickEvent)`.
  - `pub struct Stats { pub aggregates: Aggregates, pub recent: Vec<ClickEvent> }` (derive Serialize).
  - `pub fn device_from_ua(ua: Option<&str>) -> &'static str`.
  - `pub fn day_bucket(ts: u64) -> String` (YYYY-MM-DD em UTC a partir do epoch secs).
  - `#[async_trait::async_trait] pub trait AnalyticsSink: Send + Sync + 'static { async fn record_batch(&self, events: &[ClickEvent]) -> Result<(), StoreError>; async fn stats(&self, id: u64) -> Result<Option<Stats>, StoreError>; }`

- [ ] **Step 1: Escrever os testes que falham**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn ev(id: u64, ts: u64, country: &str, ua: &str) -> ClickEvent {
        ClickEvent {
            id,
            ts,
            referer: None,
            country: Some(country.into()),
            user_agent: Some(ua.into()),
        }
    }

    #[test]
    fn agrega_total_dia_pais_device() {
        let mut a = Aggregates::default();
        // 2026-07-12 e 2026-07-13 (epoch secs aproximados; day_bucket deriva a data)
        a.apply(&ev(1, 1_752_300_000, "BR", "Mozilla/5.0 (iPhone)"));
        a.apply(&ev(1, 1_752_300_050, "BR", "Mozilla/5.0 (Windows NT 10.0)"));
        a.apply(&ev(1, 1_752_400_000, "US", "curl/8.0"));
        assert_eq!(a.total, 3);
        assert_eq!(a.first_ts, 1_752_300_000);
        assert_eq!(a.last_ts, 1_752_400_000);
        assert_eq!(a.per_country.get("BR"), Some(&2));
        assert_eq!(a.per_country.get("US"), Some(&1));
        assert_eq!(a.per_device.get("Mobile"), Some(&1));
        assert_eq!(a.per_device.get("Desktop"), Some(&1));
        assert_eq!(a.per_device.get("Other"), Some(&1));
        assert_eq!(a.per_day.values().sum::<u64>(), 3);
    }

    #[test]
    fn device_heuristica() {
        assert_eq!(device_from_ua(Some("Mozilla/5.0 (iPhone; CPU iPhone OS)")), "Mobile");
        assert_eq!(device_from_ua(Some("Mozilla/5.0 (Linux; Android 14)")), "Mobile");
        assert_eq!(device_from_ua(Some("Mozilla/5.0 (Windows NT 10.0; Win64)")), "Desktop");
        assert_eq!(device_from_ua(Some("Mozilla/5.0 (Macintosh; Intel Mac OS X)")), "Desktop");
        assert_eq!(device_from_ua(Some("curl/8.0")), "Other");
        assert_eq!(device_from_ua(None), "Other");
    }

    #[test]
    fn day_bucket_datas_conhecidas() {
        assert_eq!(day_bucket(0), "1970-01-01");
        assert_eq!(day_bucket(1_735_689_600), "2025-01-01"); // epoch de 2025-01-01 00:00 UTC
        assert_eq!(day_bucket(1_735_689_600 + 86_400), "2025-01-02");
    }
}
```

- [ ] **Step 2: Rodar e confirmar falha**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test analytics`
Expected: FAIL (símbolos não existem).

- [ ] **Step 3: Implementar o núcleo puro**

```rust
use crate::store::StoreError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClickEvent {
    pub id: u64,
    pub ts: u64,
    pub referer: Option<String>,
    pub country: Option<String>,
    pub user_agent: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Aggregates {
    pub total: u64,
    pub first_ts: u64,
    pub last_ts: u64,
    pub per_day: BTreeMap<String, u64>,
    pub per_country: BTreeMap<String, u64>,
    pub per_device: BTreeMap<String, u64>,
}

impl Aggregates {
    pub fn apply(&mut self, ev: &ClickEvent) {
        self.total += 1;
        if self.first_ts == 0 || ev.ts < self.first_ts {
            self.first_ts = ev.ts;
        }
        if ev.ts > self.last_ts {
            self.last_ts = ev.ts;
        }
        *self.per_day.entry(day_bucket(ev.ts)).or_insert(0) += 1;
        if let Some(c) = &ev.country {
            *self.per_country.entry(c.clone()).or_insert(0) += 1;
        }
        let dev = device_from_ua(ev.user_agent.as_deref());
        *self.per_device.entry(dev.to_string()).or_insert(0) += 1;
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub aggregates: Aggregates,
    pub recent: Vec<ClickEvent>,
}

/// Heurística leve de device a partir do User-Agent (sem dep externa).
pub fn device_from_ua(ua: Option<&str>) -> &'static str {
    match ua {
        Some(s) => {
            let s = s.to_ascii_lowercase();
            if s.contains("iphone") || s.contains("android") || s.contains("mobile") {
                "Mobile"
            } else if s.contains("windows") || s.contains("macintosh") || s.contains("x11") || s.contains("linux") {
                "Desktop"
            } else {
                "Other"
            }
        }
        None => "Other",
    }
}

/// YYYY-MM-DD (UTC) a partir de epoch secs, via cálculo de dias (sem chrono).
pub fn day_bucket(ts: u64) -> String {
    let days = (ts / 86_400) as i64; // dias desde 1970-01-01
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

// Algoritmo de Howard Hinnant (days -> Y/M/D proléptico gregoriano).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[async_trait::async_trait]
pub trait AnalyticsSink: Send + Sync + 'static {
    async fn record_batch(&self, events: &[ClickEvent]) -> Result<(), StoreError>;
    async fn stats(&self, id: u64) -> Result<Option<Stats>, StoreError>;
}
```

Adicionar `pub mod analytics;` em `src/lib.rs`.

- [ ] **Step 4: Rodar e confirmar passa**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test analytics`
Expected: PASS (3 testes). Rodar `cargo fmt` e `cargo clippy --all-targets -- -D warnings` (limpar se necessário — ex.: `to_string()` em `&'static str` é ok; clippy pode sugerir ajustes).

- [ ] **Step 5: Commit**

```bash
git add src/analytics.rs src/lib.rs
git commit -m "feat(analytics): núcleo puro — ClickEvent, Aggregates::apply, device_from_ua, day_bucket, trait AnalyticsSink"
```

---

### Task 2: `LmdbAnalyticsSink` — sink embutido no mesmo env LMDB

**Files:**
- Modify: `src/store/lmdb.rs` (env `max_dbs=5`, DBs `stats`/`events`, `impl AnalyticsSink for LmdbStore`)
- Modify: `src/store/mod.rs` (factory `open_backends`)
- Test: `tests/analytics_sink_it.rs`

**Interfaces:**
- Consumes: `crate::analytics::{AnalyticsSink, ClickEvent, Aggregates, Stats, EVENTS_MAX}`; heed.
- Produces:
  - `LmdbStore` passa a abrir com `max_dbs(5)` e criar os DBs `stats` (`BeU64 → Bytes`, Aggregates json) e `events` (`BeU64 → Bytes`, `Vec<ClickEvent>` json).
  - `impl AnalyticsSink for LmdbStore`.
  - `pub const EVENTS_MAX: usize = 1000;` (em `analytics.rs`).
  - `pub fn open_backends(path: &Path) -> Result<(Arc<dyn Store>, Arc<dyn AnalyticsSink>), StoreError>` em `store/mod.rs` — abre UM `LmdbStore` e devolve o mesmo `Arc` coagido nos dois traits (env compartilhado).

- [ ] **Step 1: Escrever o teste de integração que falha**

```rust
// tests/analytics_sink_it.rs
use quark::analytics::{AnalyticsSink, ClickEvent};
use quark::store::open_backends;

fn ev(id: u64, ts: u64) -> ClickEvent {
    ClickEvent { id, ts, referer: None, country: Some("BR".into()), user_agent: Some("iPhone".into()) }
}

#[tokio::test]
async fn record_e_stats() {
    let dir = tempfile::tempdir().unwrap();
    let (_store, sink) = open_backends(dir.path()).unwrap();

    sink.record_batch(&[ev(1, 1_752_300_000), ev(1, 1_752_300_050)]).await.unwrap();
    let s = sink.stats(1).await.unwrap().unwrap();
    assert_eq!(s.aggregates.total, 2);
    assert_eq!(s.aggregates.per_country.get("BR"), Some(&2));
    assert_eq!(s.recent.len(), 2);
    assert!(sink.stats(999).await.unwrap().is_none());
}

#[tokio::test]
async fn retencao_trunca_em_events_max() {
    let dir = tempfile::tempdir().unwrap();
    let (_store, sink) = open_backends(dir.path()).unwrap();
    // Grava 1200 eventos pro mesmo id em lotes; recent deve ficar em 1000.
    for batch in 0..12 {
        let evs: Vec<ClickEvent> = (0..100).map(|i| ev(7, 1_752_300_000 + batch * 100 + i)).collect();
        sink.record_batch(&evs).await.unwrap();
    }
    let s = sink.stats(7).await.unwrap().unwrap();
    assert_eq!(s.aggregates.total, 1200);
    assert_eq!(s.recent.len(), 1000); // últimos N
    // o mais recente sobreviveu; o mais antigo foi truncado
    assert_eq!(s.recent.last().unwrap().ts, 1_752_300_000 + 11 * 100 + 99);
}
```

- [ ] **Step 2: Rodar e confirmar falha**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --test analytics_sink_it`
Expected: FAIL (`open_backends`/impl não existem).

- [ ] **Step 3: Implementar**

Em `analytics.rs`, adicionar `pub const EVENTS_MAX: usize = 1000;`.

Em `src/store/lmdb.rs`:
- Trocar `max_dbs(3)` → `max_dbs(5)`; no `open`, criar também `stats` e `events` (ambos `Database<BeU64, Bytes>`), guardar nos campos da struct.
- Implementar o sink:

```rust
use crate::analytics::{AnalyticsSink, Aggregates, ClickEvent, Stats, EVENTS_MAX};

#[async_trait::async_trait]
impl AnalyticsSink for LmdbStore {
    async fn record_batch(&self, events: &[ClickEvent]) -> Result<(), StoreError> {
        if events.is_empty() {
            return Ok(());
        }
        // Agrupa por id em memória pra minimizar leituras/escritas.
        use std::collections::BTreeMap;
        let mut by_id: BTreeMap<u64, Vec<&ClickEvent>> = BTreeMap::new();
        for e in events {
            by_id.entry(e.id).or_default().push(e);
        }
        let mut wtxn = self.env.write_txn()?;
        for (id, evs) in by_id {
            // agregados: lê-modifica-grava
            let mut agg: Aggregates = match self.stats.get(&wtxn, &id)? {
                Some(b) => serde_json::from_slice(b)?,
                None => Aggregates::default(),
            };
            for e in &evs {
                agg.apply(e);
            }
            self.stats.put(&mut wtxn, &id, &serde_json::to_vec(&agg)?)?;

            // eventos crus: append + trunca aos últimos EVENTS_MAX
            let mut recent: Vec<ClickEvent> = match self.events.get(&wtxn, &id)? {
                Some(b) => serde_json::from_slice(b)?,
                None => Vec::new(),
            };
            for e in &evs {
                recent.push((*e).clone());
            }
            if recent.len() > EVENTS_MAX {
                let drop = recent.len() - EVENTS_MAX;
                recent.drain(0..drop);
            }
            self.events.put(&mut wtxn, &id, &serde_json::to_vec(&recent)?)?;
        }
        wtxn.commit()?;
        Ok(())
    }

    async fn stats(&self, id: u64) -> Result<Option<Stats>, StoreError> {
        let rtxn = self.env.read_txn()?;
        let agg = match self.stats.get(&rtxn, &id)? {
            Some(b) => serde_json::from_slice::<Aggregates>(b)?,
            None => return Ok(None),
        };
        let recent: Vec<ClickEvent> = match self.events.get(&rtxn, &id)? {
            Some(b) => serde_json::from_slice(b)?,
            None => Vec::new(),
        };
        Ok(Some(Stats { aggregates: agg, recent }))
    }
}
```

Em `src/store/mod.rs`, adicionar o factory combinado:

```rust
use crate::analytics::AnalyticsSink;

/// Abre UM LmdbStore e o expõe como Store E AnalyticsSink (mesmo env LMDB).
pub fn open_backends(path: &Path) -> Result<(Arc<dyn Store>, Arc<dyn AnalyticsSink>), StoreError> {
    let backend = Arc::new(lmdb::LmdbStore::open(path)?);
    let store: Arc<dyn Store> = backend.clone();
    let sink: Arc<dyn AnalyticsSink> = backend;
    Ok((store, sink))
}
```
(Manter `open_store` como está; `main` migra pro `open_backends` na Task 4.)

- [ ] **Step 4: Rodar e confirmar passa**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --test analytics_sink_it`
Expected: PASS (2 testes). `cargo test` completo verde; `cargo fmt` + `clippy -D warnings` limpos.

- [ ] **Step 5: Commit**

```bash
git add src/store/lmdb.rs src/store/mod.rs src/analytics.rs tests/analytics_sink_it.rs
git commit -m "feat(analytics): LmdbAnalyticsSink no env compartilhado (DBs stats/events, max_dbs=5) + open_backends"
```

---

### Task 3: Worker de fundo (batch + drain-on-close)

**Files:**
- Modify: `src/analytics.rs` (add `spawn_worker`)
- Test: dentro de `src/analytics.rs` (`#[cfg(test)]`, usa `open_backends` via `tempfile`)

**Interfaces:**
- Consumes: `tokio::sync::mpsc`, `AnalyticsSink`.
- Produces:
  - `pub const BATCH: usize = 500;`
  - `pub fn spawn_worker(rx: tokio::sync::mpsc::Receiver<ClickEvent>, sink: Arc<dyn AnalyticsSink>) -> tokio::task::JoinHandle<()>`
  - Comportamento: acumula eventos; faz flush (`sink.record_batch`) quando o buffer atinge `BATCH`, a cada 5s, ou quando o canal fecha (todos os `Sender` dropados) — nesse caso drena o resto, faz flush e encerra. Erros de `record_batch` são logados (uma linha JSON no stderr) e não derrubam o worker.

- [ ] **Step 1: Escrever o teste que falha**

```rust
// dentro de #[cfg(test)] mod tests em analytics.rs
#[tokio::test]
async fn worker_drena_e_grava_ao_fechar_canal() {
    let dir = tempfile::tempdir().unwrap();
    let (_store, sink) = crate::store::open_backends(dir.path()).unwrap();
    let (tx, rx) = tokio::sync::mpsc::channel::<ClickEvent>(1000);
    let handle = spawn_worker(rx, sink.clone());

    for i in 0..250u64 {
        tx.send(ClickEvent {
            id: 5,
            ts: 1_752_300_000 + i,
            referer: None,
            country: Some("BR".into()),
            user_agent: Some("iPhone".into()),
        })
        .await
        .unwrap();
    }
    drop(tx); // fecha o canal → worker drena, faz flush e encerra
    handle.await.unwrap();

    let s = sink.stats(5).await.unwrap().unwrap();
    assert_eq!(s.aggregates.total, 250);
    assert_eq!(s.recent.len(), 250);
}
```

- [ ] **Step 2: Rodar e confirmar falha**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test analytics::tests::worker_drena`
Expected: FAIL (`spawn_worker` não existe).

- [ ] **Step 3: Implementar o worker**

```rust
use std::sync::Arc;
use tokio::sync::mpsc::Receiver;

pub const BATCH: usize = 500;

pub fn spawn_worker(
    mut rx: Receiver<ClickEvent>,
    sink: Arc<dyn AnalyticsSink>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut buf: Vec<ClickEvent> = Vec::with_capacity(BATCH);
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(5));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                maybe = rx.recv() => {
                    match maybe {
                        Some(ev) => {
                            buf.push(ev);
                            if buf.len() >= BATCH {
                                flush(&sink, &mut buf).await;
                            }
                        }
                        None => {
                            // canal fechado: drena o resto e encerra
                            flush(&sink, &mut buf).await;
                            break;
                        }
                    }
                }
                _ = ticker.tick() => {
                    flush(&sink, &mut buf).await;
                }
            }
        }
    })
}

async fn flush(sink: &Arc<dyn AnalyticsSink>, buf: &mut Vec<ClickEvent>) {
    if buf.is_empty() {
        return;
    }
    if let Err(e) = sink.record_batch(buf).await {
        eprintln!("{}", serde_json::json!({"analytics_flush_error": e.to_string()}));
    }
    buf.clear();
}
```

- [ ] **Step 4: Rodar e confirmar passa**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test analytics`
Expected: PASS (todos os testes de analytics, incl. o do worker). `cargo test` completo verde; fmt + clippy limpos.

- [ ] **Step 5: Commit**

```bash
git add src/analytics.rs
git commit -m "feat(analytics): worker de fundo (batch 500 / 5s / drain-on-close), erros logados sem afetar redirect"
```

---

### Task 4: Wiring na API + main (captura no 302, endpoint /stats, worker no startup)

**Files:**
- Modify: `src/api.rs` (redirect emite `try_send`; rota+handler `GET /:code/stats`; `AppState`)
- Modify: `src/main.rs` (canal, worker, token, `open_backends`)
- Test: `tests/analytics_api_it.rs`

**Interfaces:**
- Consumes: `analytics::{ClickEvent, AnalyticsSink, day_bucket}`, `tokio::sync::mpsc::Sender`.
- Produces:
  - `AppState { pub cache: Cache, pub store: Arc<dyn Store>, pub key: u64, pub analytics_tx: tokio::sync::mpsc::Sender<ClickEvent>, pub sink: Arc<dyn AnalyticsSink>, pub admin_token: Option<String> }`
  - Rota `GET /:code/stats`.

- [ ] **Step 1: Escrever os testes de integração que falham**

```rust
// tests/analytics_api_it.rs
use axum::body::Body;
use axum::http::{Request, StatusCode};
use quark::analytics::AnalyticsSink;
use quark::api::{router, AppState};
use quark::cache::Cache;
use quark::store::open_backends;
use std::sync::Arc;
use tower::ServiceExt;

fn app_with(admin: Option<&str>, chan_cap: usize) -> (axum::Router, tokio::sync::mpsc::Receiver<quark::analytics::ClickEvent>) {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let (store, sink) = open_backends(dir.path()).unwrap();
    let cache = Cache::new(store.clone(), 1000);
    let (tx, rx) = tokio::sync::mpsc::channel(chan_cap);
    let state = Arc::new(AppState {
        cache,
        store,
        key: 0x1234,
        analytics_tx: tx,
        sink,
        admin_token: admin.map(|s| s.to_string()),
    });
    (router(state), rx)
}

async fn create(app: &axum::Router, url: &str) -> String {
    let resp = app.clone().oneshot(
        Request::post("/").header("content-type", "application/json")
            .body(Body::from(format!(r#"{{"url":"{url}"}}"#))).unwrap()
    ).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    v["code"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn redirect_nao_bloqueia_com_fila_cheia() {
    // canal capacidade 1, SEM worker consumindo: enche na 1ª e descarta o resto,
    // mas o redirect precisa continuar respondendo 302.
    let (app, _rx) = app_with(None, 1);
    let code = create(&app, "https://example.com").await;
    for _ in 0..5 {
        let resp = app.clone().oneshot(Request::get(format!("/{code}")).body(Body::empty()).unwrap())
            .await.unwrap();
        assert_eq!(resp.status(), StatusCode::FOUND); // 302 sempre, mesmo com fila cheia
    }
}

#[tokio::test]
async fn stats_exige_token() {
    let (app, _rx) = app_with(Some("segredo"), 100);
    let code = create(&app, "https://example.com").await;
    // sem token → 401
    let resp = app.clone().oneshot(Request::get(format!("/{code}/stats")).body(Body::empty()).unwrap())
        .await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    // com token errado → 401
    let resp = app.clone().oneshot(
        Request::get(format!("/{code}/stats")).header("x-admin-token", "errado").body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    // com token certo → 200
    let resp = app.clone().oneshot(
        Request::get(format!("/{code}/stats")).header("x-admin-token", "segredo").body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn stats_desligado_sem_token_configurado() {
    let (app, _rx) = app_with(None, 100); // admin_token None
    let code = create(&app, "https://example.com").await;
    let resp = app.clone().oneshot(
        Request::get(format!("/{code}/stats")).header("x-admin-token", "qualquer").body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
```

Garantir `tower` em `[dev-dependencies]` (já está do Tijolo do núcleo).

- [ ] **Step 2: Rodar e confirmar falha**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --test analytics_api_it`
Expected: FAIL (campos de `AppState`/rota não existem).

- [ ] **Step 3: Implementar no `src/api.rs`**

- Estender `AppState` com `analytics_tx: tokio::sync::mpsc::Sender<ClickEvent>`, `sink: Arc<dyn AnalyticsSink>`, `admin_token: Option<String>`.
- No handler `redirect`, no ramo de sucesso **302** (link vivo, antes de retornar a resposta), emitir o evento **sem bloquear**:

```rust
// dentro do redirect, no ponto onde já se sabe que vai retornar 302 pro `rec`:
let ev = ClickEvent {
    id,
    ts: now(),
    referer: req_headers.get(axum::http::header::REFERER).and_then(|v| v.to_str().ok()).map(|s| s.to_string()),
    country: req_headers.get("cf-ipcountry").and_then(|v| v.to_str().ok()).map(|s| s.to_string()),
    user_agent: req_headers.get(axum::http::header::USER_AGENT).and_then(|v| v.to_str().ok()).map(|s| s.to_string()),
};
let _ = st.analytics_tx.try_send(ev); // best-effort; fila cheia => descarta
```
Para ter os headers no handler, adicionar `headers: axum::http::HeaderMap` como extrator do `redirect` (axum injeta). NÃO alterar status/Location/Cache-Control existentes; a emissão do evento é adicional e ignora o resultado do `try_send`.

- Adicionar o handler `stats`:

```rust
async fn stats(
    State(st): State<Arc<AppState>>,
    Path(code): Path<String>,
    headers: axum::http::HeaderMap,
) -> Response {
    // endpoint desligado se não há token configurado
    let Some(expected) = st.admin_token.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let provided = headers.get("x-admin-token").and_then(|v| v.to_str().ok()).unwrap_or("");
    if !constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    // resolve code -> id (mesma lógica do redirect)
    let id = match codec::from_base62(&code) {
        Some(c) if c <= permute::MAX_ID => permute::decode(c, st.key),
        _ => match st.store.get_alias(&code).await {
            Ok(Some(id)) => id,
            Ok(None) => return StatusCode::NOT_FOUND.into_response(),
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        },
    };
    match st.sink.stats(id).await {
        Ok(Some(s)) => Json(s).into_response(),
        Ok(None) => Json(serde_json::json!({"total": 0, "aggregates": null, "recent": []})).into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}
```
- Registrar a rota no `router`: `.route("/:code/stats", get(stats))` (é um path de 2 segmentos, não conflita com `/:code`).

- [ ] **Step 4: Implementar no `src/main.rs`**

```rust
use quark::analytics::spawn_worker;
use quark::store::open_backends;
// ...
let (store, sink) = open_backends(std::path::Path::new(&path)).expect("abrir backends");
let cache = Cache::new(store.clone(), 100_000);
let (analytics_tx, analytics_rx) = tokio::sync::mpsc::channel(10_000);
let _worker = spawn_worker(analytics_rx, sink.clone());
let admin_token = std::env::var("QUARK_ADMIN_TOKEN").ok();
if admin_token.is_none() {
    eprintln!("AVISO: QUARK_ADMIN_TOKEN não definido — endpoint /stats desligado.");
}
let state = Arc::new(AppState { cache, store, key, analytics_tx, sink, admin_token });
```

- [ ] **Step 5: Rodar e confirmar passa**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --test analytics_api_it`
Expected: PASS (3 testes). `cargo test` completo verde; fmt + clippy `-D warnings` limpos.

- [ ] **Step 6: Smoke manual**

Run: `QUARK_DATA=./data-smoke QUARK_ADMIN_TOKEN=segredo cargo run`; noutro terminal: criar link, acessar 2x, então `curl -s -H 'x-admin-token: segredo' localhost:8080/<code>/stats` → JSON com `total` ≥ 1 (dar ~6s pro flush, ou o worker faz flush no intervalo). Parar e `rm -rf ./data-smoke`.

- [ ] **Step 7: Commit**

```bash
git add src/api.rs src/main.rs tests/analytics_api_it.rs
git commit -m "feat(analytics): captura fire-and-forget no 302 + GET /:code/stats (token) + worker no startup"
```

---

## Self-Review (feito pelo autor do plano)

**Cobertura do spec:**
- Impacto zero (try_send drop-on-full) → Task 4 Step 3 + teste `redirect_nao_bloqueia_com_fila_cheia`. ✓
- Worker batch/5s/drain → Task 3. ✓
- AnalyticsSink trait + sink LMDB (env compartilhado, max_dbs=5, stats/events) → Tasks 1-2. ✓
- Agregados (total/dia/país/device) + últimos N → Tasks 1-2 (`apply`, retenção). ✓
- Geo via CF-IPCountry → Task 4 Step 3. ✓
- /stats protegido por token (401/404-off/200) → Task 4. ✓
- Config QUARK_ADMIN_TOKEN + constantes → Task 4 / Tasks 1-3. ✓

**Placeholders:** nenhum — todos os steps de código têm código real.

**Consistência de tipos:** `ClickEvent`/`Aggregates`/`Stats`/`AnalyticsSink`/`spawn_worker`/`open_backends`/`AppState` usados com as mesmas assinaturas entre as tasks.

**Nota:** cada task compila e passa verde ao fim (Task 1 puro; Task 2 usa Task 1 + heed; Task 3 usa Task 2; Task 4 faz o wiring). O worker é testado de forma determinística via drain-on-close (drop do Sender), evitando flakiness de timing.
