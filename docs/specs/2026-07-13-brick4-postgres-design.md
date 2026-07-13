# Tijolo 4 — Backend Postgres — design

**Data:** 2026-07-13
**Status:** spec (usuário delegou tijolos 3-5; arquitetura assentada)
**Programa:** quarto de 5 (1. storage ✅ · 2. analytics ✅ · 3. L2 Valkey ✅ · 4. Postgres ← *este* · 5. ClickHouse).

## 1. Objetivo

Um backend **Postgres** que implementa `Store` E `AnalyticsSink` (o mesmo padrão
do LMDB: uma struct, um recurso, dois traits), selecionável em runtime por
`QUARK_DATABASE_URL`. Habilita dados relacionais/multi-node — o caminho pra
contas/times/multi-instância. Sem a env, o default segue LMDB embutido (zero
mudança).

**Invariante:** o `Store`/`AnalyticsSink` já são traits async (Tijolo 1/2); o
Postgres pluga atrás deles sem tocar api/cache/worker. O redirect continua indo
via cache (L1/L2) → store; com Postgres, o "store" é uma query indexada (+ o L2
Valkey na frente absorve o read path).

## 2. Escopo

**No tijolo:**
- `PostgresStore` (sqlx + pool) implementando `Store` (links, aliases, contador
  de id) e `AnalyticsSink` (stats, events).
- Schema idempotente criado no `open` (CREATE TABLE/SEQUENCE IF NOT EXISTS) — sem
  ferramenta de migração externa por ora.
- Factory: `open_backends` seleciona LMDB vs Postgres por `QUARK_DATABASE_URL`.
- Generalizar `StoreError` pra carregar erro não-heed (o Postgres traz `sqlx::Error`).
- Testes de integração gated (`QUARK_TEST_DATABASE_URL`); CI com serviço Postgres.

**Fora:**
- Contas/times/schema de usuário (fase de produto).
- Migrações versionadas (idempotente basta pro v1).
- Sharding/replica (multi-node é possível, mas orquestração fica pra depois).

## 3. Modelo relacional

```sql
CREATE SEQUENCE IF NOT EXISTS quark_id_seq;              -- next_id atômico
CREATE TABLE IF NOT EXISTS links (
  id BIGINT PRIMARY KEY,
  url TEXT NOT NULL,
  expiry BIGINT,          -- epoch secs, NULL = sem expiração
  created BIGINT NOT NULL
);
CREATE TABLE IF NOT EXISTS aliases (
  alias TEXT PRIMARY KEY,
  id BIGINT NOT NULL
);
CREATE TABLE IF NOT EXISTS stats (
  id BIGINT PRIMARY KEY,
  agg JSONB NOT NULL      -- Aggregates serializado (reusa o mesmo shape)
);
CREATE TABLE IF NOT EXISTS events (
  id BIGINT PRIMARY KEY,
  recent JSONB NOT NULL   -- Vec<ClickEvent> circular (últimos N)
);
```
- **`next_id`** = `SELECT nextval('quark_id_seq')` (atômico, sem race).
- **`put_alias_and_link`** = transação: `INSERT INTO aliases ... ON CONFLICT DO NOTHING` → se 0 linhas, alias já existe → rollback/return false; senão `INSERT INTO links`.
- Agregados/events como JSONB reusam exatamente `Aggregates`/`Vec<ClickEvent>` do
  `analytics.rs` (mesma serialização serde) — read-modify-write como no LMDB, mas
  numa transação Postgres. (Escala melhor que o blob LMDB por ter índice por PK e,
  no futuro, dá pra migrar `events` pra linhas; v1 mantém o JSONB pela simetria.)

## 4. `StoreError` generalizado

Hoje `StoreError { Db(heed::Error), Serde(serde_json::Error) }`. Adicionar uma
variante genérica pra backends não-LMDB:
```rust
pub enum StoreError { Db(heed::Error), Serde(serde_json::Error), Backend(String) }
```
O Postgres mapeia `sqlx::Error` → `StoreError::Backend(e.to_string())`. (Alternativa
mais limpa — trocar tudo por `Backend(String)` — evitada pra não churnar o LMDB;
`Backend` aditivo basta.)

## 5. Factory / seleção de backend

```rust
pub async fn open_backends(cfg) -> Result<(Arc<dyn Store>, Arc<dyn AnalyticsSink>), StoreError> {
    match std::env::var("QUARK_DATABASE_URL") {
        Ok(url) => { let pg = Arc::new(PostgresStore::open(&url).await?); (pg.clone(), pg) }
        Err(_)  => { let lmdb = Arc::new(LmdbStore::open(path)?); (lmdb.clone(), lmdb) }
    }
}
```
`open_backends` passa a receber o `data_path` (pro LMDB) e ler a env do Postgres.
Assinatura ajustada; `main` passa o path.

## 6. sqlx

- `sqlx = { version = "0.8", features = ["runtime-tokio", "postgres", "macros"] }`
  usando as APIs **runtime** (`query`/`query_as` com bind), **não** as macros
  compile-time (`query!`) — pra não exigir um Postgres no momento da compilação/CI
  de build. Pool: `sqlx::postgres::PgPoolOptions`.
- `PostgresStore { pool: PgPool }`.

## 7. Config nova

- `QUARK_DATABASE_URL` (ex.: `postgres://user:pass@host:5432/quark`). Ausente →
  LMDB (default). Presente → Postgres.

## 8. Arquivos

- Novo: `src/store/postgres.rs` (`PostgresStore`, impl `Store` + `AnalyticsSink`).
- `src/store/mod.rs`: `StoreError::Backend`; `open_backends` com seleção por env.
- `src/main.rs`: chamar `open_backends` com o path; log do backend escolhido.
- `Cargo.toml`: sqlx.
- CI: serviço Postgres + `QUARK_TEST_DATABASE_URL`.

## 9. Testes

- **Integração gated (`QUARK_TEST_DATABASE_URL`), validado contra Postgres Docker:**
  - Store: put/get link; next_id incrementa (sequência); alias não sobrescreve;
    put_alias_and_link atômico (sem órfão).
  - AnalyticsSink: record_batch + stats; retenção trunca em N.
  - Cada teste usa um schema/tabelas limpas (DROP/CREATE ou TRUNCATE no setup, ou
    um DB de teste dedicado) pra isolar.
- **Unit (sem serviço):** o `StoreError::Backend` Display/From; qualquer helper puro.
- Os testes existentes (LMDB) seguem rodando sem Postgres.

## 10. CI

Serviço `postgres:16` no job, `QUARK_TEST_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/postgres`, e rodar os gated. Testes unit + LMDB seguem sem serviço.

## 11. Riscos / notas

- **Read path com Postgres:** uma query por PK é rápida, mas é rede. Por isso o L2
  Valkey (Tijolo 3) importa numa deploy Postgres — absorve os reads quentes. O
  redirect continua L1→L2→store(Postgres).
- **next_id via sequência:** atômico e multi-node-safe (ao contrário do contador
  LMDB single-node). Bônus real do Postgres.
- **JSONB read-modify-write pra analytics:** mesma amplificação do LMDB; aceitável
  no v1. ClickHouse (Tijolo 5) é o alvo pra analytics em volume.
- **Compile sem Postgres:** usando as APIs runtime do sqlx (não `query!` macro),
  o build/CI-de-build não exige um Postgres.
- **`sqlx::Error` no request path:** mapeado pra `StoreError::Backend` e tratado
  como os demais (503 na api). Sem panic.
