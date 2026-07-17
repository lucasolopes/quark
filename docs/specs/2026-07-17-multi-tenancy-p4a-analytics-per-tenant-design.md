# P4a — Analytics por tenant (tenant_id nos eventos + agregado) (cloud-aware)

**Status:** design. Parte independente-de-decisão do P4 (LUC-9): funciona no backend Postgres atual, prepara o schema do ClickHouse pro P4b (provisão = ação do usuário). Base `main @ f4a9b51`.

## Objetivo

Marcar cada evento de clique com o `tenant_id` do link e permitir analytics **agregado por tenant** ("todos os meus links"), não só por link. Hoje o isolamento é implícito (um tenant só consulta stats de links que possui, via posse), mas não dá pra agregar cross-link por tenant porque o evento não carrega `tenant_id`.

## Contexto (do mapa)

- `ClickEvent` (`src/analytics/mod.rs:11-44`) NÃO tem `tenant_id`. Construído no redirect (`src/api.rs:1304-1326`) onde `route.tenant_id` e `rec.tenant_id` estão em escopo (pós-filtro de isolamento; `rec` é autoritativo). Vai por `analytics_tx.try_send` (`:1346`) → `spawn_worker`/`flush` (`mod.rs:304-387`, batelada, agnóstico) → `sink.record_batch`.
- **Postgres já tem `tenant_id`** em `click_events`/`click_counters`/`stats_meta` (via `TENANT_OWNED_TABLES` + o loop genérico `ADD COLUMN tenant_id DEFAULT 0`, `postgres.rs:650-656`) — mas **morto**: os 3 INSERTs (`postgres.rs:2530-2569`) não bindam, os 3 SELECTs (`:2586,2627,2638`) filtram só `id`. Índice `click_counters_by_tenant` (`:760`) existe mas inútil hoje. NOT_FORCED (worker no pool pelado) → isolamento é app-level `WHERE tenant_id`.
- `AnalyticsSink` trait (`mod.rs:271-275`): `record_batch(&[ClickEvent])`, `stats(id)`. `ClickHouseSink` (`src/analytics/clickhouse.rs`): `ClickRow` (`:9-23`, sem tenant_id), DDL `ORDER BY (id, ts)` (`:104`), migração `ALTER ADD COLUMN IF NOT EXISTS` (`:113-137`). Sink escolhido em `open_backends` (`store/mod.rs:938-977`), um global via `QUARK_CLICKHOUSE_URL`.
- `stats` handler `GET /:code/stats` (`api.rs:1366`): `admin_guard(Analytics)` → posse via `get_link(p.tenant, id)` → `sink.stats(id)` (sem tenant).

## Arquitetura

### 1. `tenant_id` no ClickEvent + stamp
`ClickEvent` ganha `tenant_id: u64` (`#[serde(default)]` pra compat de blobs antigos). No redirect, carimbar `tenant_id: rec.tenant_id.0` na construção. Worker/flush inalterados (só carregam o campo novo).

### 2. Postgres sink popula + agrega
- `record_batch`: bindar `tenant_id` nos 3 INSERTs (`click_counters`/`stats_meta`/`click_events`).
- `stats(id)`: manter keyed por `id` (per-link); opcionalmente `AND tenant_id` como defesa-em-profundidade (o `id` já é global e a posse é checada no handler — deixar como está é aceitável; não regredir).
- **Novo** `stats_for_tenant(tenant) -> Aggregates` (trait method): agrega `click_events`/`click_counters` `WHERE tenant_id = $1` (sem `recent` per-link; só os agregados). 
- **Backfill** (migração no boot, uma vez): `UPDATE click_events ce SET tenant_id = l.tenant_id FROM links l WHERE l.id = ce.id AND ce.tenant_id = 0` (idem `click_counters`/`stats_meta`). Corrige as linhas existentes mis-taggeadas como 0. (Prod ainda não no ar → sem dados reais, mas fica correto pro futuro.) Cliques de link deletado ficam em 0 (órfãos) — aceitável.

### 3. Endpoint agregado `/admin/stats` (todos os meus links)
`GET /admin/stats` cloud-aware: `admin_guard(Analytics)` → `sink.stats_for_tenant(p.tenant)` → `Aggregates` do tenant. No OSS = tenant 0 (agrega tudo, como esperado). Habilita o LUC-45 (visão de analytics dedicada) no futuro.

### 4. ClickHouse sink (compile-ready pro P4b)
- `ClickRow` ganha `tenant_id: u64`; `record_batch` carimba `e.tenant_id`.
- DDL: `ALTER TABLE clicks ADD COLUMN IF NOT EXISTS tenant_id UInt64 DEFAULT 0` (idempotente, padrão existente) + a chave: `ORDER BY (tenant_id, id, ts)` na criação (uma instância, particionada por tenant — decisão do usuário). 
- `stats(id)` + `stats_for_tenant(tenant)` com predicado de tenant.
- Backfill do ClickHouse = job separado (mutations async) — **deferido pro P4b** (quando o servidor existir); anotar. P4a deixa o código de escrita/schema pronto; validação real contra ClickHouse é no P4b.

## Escopo
**Dentro:** `tenant_id` no `ClickEvent` + stamp; Postgres sink bind + backfill + `stats_for_tenant`; `/admin/stats` agregado; ClickHouse sink schema/row/record_batch/stats (compile-ready). Testado no Postgres (backend atual).
**Fora:** provisionar o servidor ClickHouse (P4b, infra do usuário); backfill do ClickHouse (P4b); UI de analytics dedicada (LUC-45); mudar o `stats` per-link (fica).

## Testes
- Evento carimbado: um clique num link de tenant B grava `click_events.tenant_id = B` (não 0). 
- Agregado: `stats_for_tenant(B)` conta só os cliques dos links de B; `stats_for_tenant(A)` não vê os de B. `/admin/stats` como Principal de B → agregado de B.
- Backfill: linhas pré-existentes (tenant 0) viram o tenant do link via `links.id`; idempotente (roda 2x, não muda de novo). Link deletado → fica 0.
- per-link `stats(id)` inalterado (regressão).
- Paridade OSS: tenant 0, agrega tudo, comportamento atual.
- ClickHouse: compila; DDL/row/queries com tenant_id (validação real deferida ao P4b).
- Postgres gated não-superuser; `-j1`; sem CONCURRENTLY.

## Riscos
1. **Linhas existentes mis-taggeadas como 0** (a coluna existe morta). Mitigação: backfill de `links.tenant_id` por `id` no boot, idempotente. Prod vazio hoje.
2. **NOT_FORCED nas tabelas de analytics** → isolamento do agregado é 100% app-level `WHERE tenant_id`. Mitigação: o predicado tem que estar em TODA query do `stats_for_tenant`; teste de isolamento cross-tenant.
3. **ClickHouse não testável in-process** (sem servidor) → código compile-ready, validação no P4b. Anotar claramente.
4. **serde compat** do `ClickEvent` (blobs em cache/recent) → `#[serde(default)]` no campo novo.
