# Avaliação/decisão — P4 Integrações & analytics por tenant (LUC-9)

**Status:** parcialmente já feito; o resto tem decisão de infra/custo sua. Levantado 2026-07-17 /loop.

## O que JÁ está feito (via carry-overs P1b/P2a)

- **Google Sheets por tenant: PRONTO.** `sheets_connect`/`callback`/`status`/`sync`/`disconnect` já operam por `p.tenant` (`src/api.rs:2512,2551,2600,2628,2658`); `sheets_connection` PK = `tenant_id` (migrado); o OAuth `state` carrega o tenant assinado (LUC-28, Done). Ou seja, a parte "Sheets como extensão OAuth por tenant" do LUC-9 está essencialmente entregue — cada tenant conecta a própria planilha. Falta só, se quiser, a UI de extensões por-tenant (LUC-14, backlog separado).

## O que falta — e as decisões

- **ClickHouse: decisão de infra/custo sua.** Hoje `ClickHouseSink::open(QUARK_CLICKHOUSE_URL)` é um sink de analytics OPCIONAL (só CI/dev; prod usa Postgres pra `click_events`/`click_counters`). O LUC-9 pede "ClickHouse por tenant + provisionar servidor". Decisões:
  1. **Provisionar um ClickHouse de prod?** É custo/infra novo. Alternativa: manter analytics no Postgres (funciona hoje) e adiar o ClickHouse até volume justificar. → **sua decisão** (igual foi a do edge worker).
  2. **"Por tenant" = como?** Quase certo: **um** ClickHouse, linhas marcadas por `tenant_id` (partição/filtro), NÃO uma instância por tenant (caro demais). Confirmar.

- **Particionar analytics por tenant — chunk implementável (decision-independent):** hoje o `ClickEvent`/`click_events` provavelmente NÃO carrega `tenant_id` (analytics é keyed por link id). Isolamento atual é **implícito**: um tenant só consulta `GET /:code/stats` de links que ele possui (via `admin_guard` + posse do link), então não vê clicks de outro. MAS uma visão agregada "total de cliques dos meus links" precisa de `tenant_id` no evento. Adicionar `tenant_id` ao `ClickEvent` + aos inserts/queries (Postgres hoje, ClickHouse depois) é decidível e vale — funciona nos dois backends. Isso pode virar uma sub-fase P4a (analytics tenant-aware no Postgres) independente da decisão do ClickHouse.

## Recomendação

- **P4a (fazer quando priorizar):** marcar `ClickEvent` com `tenant_id` + queries agregadas por tenant, no Postgres (backend atual). Decision-independent, entrega "analytics por tenant" de verdade sem custo de infra novo.
- **P4b (sua decisão):** provisionar ClickHouse de prod (custo) — só quando o volume justificar; até lá o Postgres cobre. Um ClickHouse, particionado por `tenant_id`.
- **Sheets:** considerar o item do LUC-9 quase fechado; a parte que sobra é UI (LUC-14).

Não comecei o P4 — depende da sua direção (provisionar ClickHouse ou não; fazer o P4a de Postgres primeiro). Enquanto isso, completei/estou completando o que NÃO depende de decisão (frontend do P2c).
