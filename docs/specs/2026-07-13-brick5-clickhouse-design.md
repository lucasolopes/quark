# Tijolo 5 — Sink ClickHouse — design

**Data:** 2026-07-13
**Status:** spec (usuário delegou tijolos 3-5; arquitetura assentada)
**Programa:** quinto de 5 (1. storage ✅ · 2. analytics ✅ · 3. L2 Valkey ✅ · 4. Postgres ✅ · 5. ClickHouse ← *este*).

## 1. Objetivo

Um `AnalyticsSink` **ClickHouse** pra analytics de clique em **alto volume** — o
padrão de mercado (Cloudflare/Dub-via-Tinybird). ClickHouse é OLAP colunar,
append-otimizado: em vez do read-modify-write dos sinks embutido/Postgres, ele
**ingere eventos crus (INSERT)** e **calcula agregados por query (GROUP BY)**.
Opt-in via `QUARK_CLICKHOUSE_URL`, **independente** do backend de Store.

**Ponto-chave (recomendação do review do Tijolo 4):** a seleção do `AnalyticsSink`
passa a ser **desacoplada** da do `Store` — ClickHouse é analytics-only. Topologia
realista: Store=LMDB/Postgres + AnalyticsSink=ClickHouse.

## 2. Escopo

**No tijolo:**
- `ClickHouseSink` (crate `clickhouse`) implementando `AnalyticsSink`.
- `record_batch` = **INSERT em lote** de eventos crus numa tabela `clicks`
  (append-only, sem update — o forte do ClickHouse).
- `stats(id)` = **queries de agregação** (count/min/max + GROUP BY por dia/país/
  device) reconstruindo o mesmo `Stats { aggregates, recent }`; `recent` = últimos
  N por `ORDER BY ts DESC LIMIT N`.
- Factory desacoplado: `open_backends` escolhe Store (`QUARK_DATABASE_URL`) e
  Sink (`QUARK_CLICKHOUSE_URL`) **separadamente**; sem a env do ClickHouse, o sink
  segue o do Store (LMDB/Postgres).
- Testes gated (`QUARK_TEST_CLICKHOUSE_URL`); CI com serviço ClickHouse.

**Fora:**
- Materialized views / rollups (agregação on-the-fly basta pro v1; MV é otimização).
- Migração de dados dos sinks antigos pro ClickHouse.
- TTL/retention no ClickHouse (o `recent` já limita via LIMIT N na query; retenção
  de storage é config do ClickHouse, fora do escopo).

## 3. Modelo (append + query)

Tabela (schema idempotente no `open`):
```sql
CREATE TABLE IF NOT EXISTS clicks (
  id        UInt64,
  ts        UInt64,             -- epoch secs
  country   String,            -- '' se ausente
  device    String,            -- Mobile/Desktop/Other (derivado no worker? ou aqui)
  referer   String             -- '' se ausente
) ENGINE = MergeTree ORDER BY (id, ts);
```
- **`record_batch`**: `INSERT INTO clicks` dos eventos (device derivado de
  `device_from_ua` reusando o `analytics.rs`; country/referer com default '').
  Append puro, um insert em lote.
- **`stats(id)`**:
  - `total/first_ts/last_ts`: `SELECT count(), min(ts), max(ts) FROM clicks WHERE id=?`
    (se count=0 → `None`).
  - `per_day`: `SELECT formatDateTime(toDate(toDateTime(ts,'UTC')),'%F') d, count() FROM clicks WHERE id=? GROUP BY d` (YYYY-MM-DD UTC, casa com `day_bucket`).
  - `per_country`: `... GROUP BY country` (mapeia '' → chave omitida? manter '' fora do mapa se vazio).
  - `per_device`: `... GROUP BY device`.
  - `recent`: `SELECT ts, country, device, referer FROM clicks WHERE id=? ORDER BY ts DESC LIMIT N` → montar `Vec<ClickEvent>` (reverter pra ordem cronológica; `country`/`referer` '' → None; `user_agent` não é re-hidratado — guardamos device, não o UA cru → `user_agent: None` no recent do ClickHouse; documentar essa diferença de fidelidade vs os outros sinks).

## 4. Desacoplar Store vs Sink no factory

```rust
pub async fn open_backends(data_path: &Path)
    -> Result<(Arc<dyn Store>, Arc<dyn AnalyticsSink>), StoreError> {
    // Store: Postgres se QUARK_DATABASE_URL, senão LMDB
    let (store, embedded_sink): (Arc<dyn Store>, Arc<dyn AnalyticsSink>) = ...; // como no Tijolo 4
    // Sink: ClickHouse se QUARK_CLICKHOUSE_URL, senão o sink do próprio store
    let sink: Arc<dyn AnalyticsSink> = match std::env::var("QUARK_CLICKHOUSE_URL") {
        Ok(url) => Arc::new(ClickHouseSink::open(&url).await?),
        Err(_)  => embedded_sink,
    };
    Ok((store, sink))
}
```
Agora Store e Sink são escolhidos por envs distintas; ClickHouse pluga só no
analytics sem tocar o store.

## 5. Crate

- `clickhouse = { version = "0.13", features = ["..."] }` (client async oficial;
  `#[derive(Row, Serialize, Deserialize)]` pras linhas). Alternativa considerada:
  HTTP puro via reqwest — preterida (o crate é purpose-built e mais limpo).
- `ClickHouseSink { client: clickhouse::Client }`.

## 6. Config nova

- `QUARK_CLICKHOUSE_URL` (ex.: `http://clickhouse:8123`). Ausente → sink do Store.

## 7. Arquivos

- Novo: `src/analytics/clickhouse.rs` (ou `src/clickhouse_sink.rs`) — `ClickHouseSink`.
  (Se `analytics` for um arquivo, pode virar dir `analytics/` com `mod.rs` +
  `clickhouse.rs`; ou um módulo top-level `src/clickhouse_sink.rs`. Escolha do plano.)
- `src/store/mod.rs`: `open_backends` desacopla Store vs Sink.
- `src/main.rs`: log do sink escolhido (nome, sem URL).
- `Cargo.toml`: clickhouse.
- CI: serviço ClickHouse + `QUARK_TEST_CLICKHOUSE_URL`.

## 8. Testes

- **Integração gated (`QUARK_TEST_CLICKHOUSE_URL`), validado contra ClickHouse Docker
  (`clickhouse/clickhouse-server`):**
  - record_batch + stats: insere eventos, `stats(id)` retorna total/por-dia/país/device
    corretos e `recent` com os últimos N. **Nota:** ClickHouse é eventual em alguns
    cenários; usar inserts síncronos (o client espera o INSERT) e, se preciso, um
    `OPTIMIZE`/pequena espera não é necessário pois MergeTree serve o SELECT logo
    após o INSERT confirmado.
  - stats de id inexistente (count=0) → None.
  - `#[serial(ch)]` se compartilharem tabela (limpar via `TRUNCATE TABLE clicks` no setup).
- **Unit:** nenhum novo puro (a lógica de agregação vive na query); os testes dos
  outros sinks seguem.

## 9. CI

Serviço `clickhouse/clickhouse-server` (porta HTTP 8123), `QUARK_TEST_CLICKHOUSE_URL=http://127.0.0.1:8123`, rodar os gated. Unit + LMDB seguem sem serviço.

## 10. Riscos / notas

- **Fidelidade do `recent`:** o ClickHouse guarda `device` (derivado), não o
  `user_agent` cru → no `recent` do ClickHouse, `user_agent` volta `None`.
  Aceitável e documentado (os agregados por device são preservados; o UA cru não é
  o dado analítico útil).
- **Modelo diferente (append vs RMW):** é o certo pro ClickHouse; o `record_batch`
  fica mais barato (INSERT puro, sem read) e o custo vai pra query do `stats` — que
  é exatamente onde o ClickHouse brilha.
- **Consistência INSERT→SELECT:** com INSERT síncrono confirmado, o SELECT logo
  depois enxerga os dados (MergeTree). Sem async-insert (que seria eventual).
- **Erro do ClickHouse no request path:** `stats` é chamado só pelo endpoint
  `/stats` (admin), não pelo redirect → mapear erro pra `StoreError::Backend` (503).
  `record_batch` roda no worker de fundo → erro logado, não afeta redirect.
- **`day_bucket` UTC:** garantir `toDateTime(ts,'UTC')` pra casar com o formato do
  `analytics::day_bucket`.
