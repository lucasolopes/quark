# LUC-63 — Worker de webhooks in-memory por tenant

Data: 2026-07-18
Issue: LUC-63 (descoberto no LUC-36)

## Diagnóstico (investigação)

Há dois caminhos de entrega de webhook:

1. **Outbox durável (Postgres/cloud)** para eventos lifecycle
   (created/updated/deleted): `WebhookDispatcher::lifecycle_deliveries(tenant,
   ev)` (delivery.rs:133) já lê `list_webhooks(tenant)` e grava linhas de
   outbox carimbadas com `tenant_id`; o relay (`spawn_webhook_relay`) entrega
   resolvendo cada linha contra `get_webhook(delivery.tenant_id, id)`. **Já é
   correto por tenant.** O `refresh_relay_snapshot(DEFAULT_TENANT)` é só um
   fast-path documentado com fallback per-tenant, não a fonte autoritativa.

2. **Worker in-memory best-effort (`spawn_webhook_worker`)** para
   `link.clicked` e `link.expired` (e `link.broken`/`link.recovered` do health
   checker). Roda SEMPRE (main.rs:275), inclusive na cloud (Postgres), porque
   clicked/expired são alto volume e deliberadamente não vão pro outbox. Este
   worker é `DEFAULT_TENANT`-only:
   - `refresh_snapshot` (delivery.rs:226) lê só `list_webhooks(DEFAULT_TENANT)`.
   - Os atomics de gate `clicked_subscribed`/`expired_subscribed` são
     calculados só das subs do tenant 0.
   - `WebhookEvent` (webhooks/mod.rs:132) = `{ event_type, body }` — **não
     carrega tenant**, então mesmo com snapshot multi-tenant não dá pra casar
     evento→tenant.

**Consequência (cloud multi-tenant):** um `link.clicked`/`link.expired` de um
link de tenant != 0 nunca dispara webhook (o gate atomic é false porque só olha
tenant 0; e mesmo se emitido, o worker só tem as subs do tenant 0). O tenant
dono está disponível no emit site (`rec.tenant_id`, api.rs:1337).

## Escopo

Tornar o worker in-memory (clicked/expired/broken/recovered) correto por
tenant. NÃO tocar no outbox/relay (já corretos). Fora de escopo: rotear
clicked/expired pro outbox (decisão de design mantida: alto volume fica
in-memory best-effort).

## Design

1. **`WebhookEvent` ganha `tenant_id: TenantId`** (webhooks/mod.rs:132).
   Todos os sites de construção passam o tenant:
   - `api.rs:1208` (expired) e `api.rs:1344` (clicked): `rec.tenant_id`.
   - `health.rs:247` (broken/recovered): o tenant do link em varredura
     (`DEFAULT_TENANT` hoje — o health checker é P-something; usar o tenant
     disponível ali, ver nota).
   - Sites de lifecycle que emitem via `emit_if_in_memory` (LMDB): o tenant do
     mutation (já conhecido no handler). Em LMDB é sempre `DEFAULT_TENANT`.
   - Testes em delivery.rs.

2. **Snapshot do worker vira `Vec<(TenantId, Vec<WebhookSubscription>)>`.**
   `refresh_snapshot` itera `list_tenants()` + `list_webhooks(t)` (padrão do
   LUC-36). Fail-open: erro ao listar tenants ou subs de um tenant mantém o
   snapshot anterior (não esvazia). Atomics:
   - `clicked_subscribed = any tenant tem sub ativa com LinkClicked`.
   - `expired_subscribed = any tenant tem sub ativa com LinkExpired`.

3. **`deliver_to_matching(subs_by_tenant, ev)`** entrega `ev` apenas às subs
   do grupo `ev.tenant_id` (isolamento cross-tenant), reusando o
   `deliver_to_matching_guarded` com o SSRF guard. Um evento de tenant A nunca
   é entregue a uma sub de tenant B.

4. **OSS/single-tenant**: `list_tenants()` devolve só o default → um grupo,
   comportamento idêntico ao de hoje.

## Testes (TDD)

Em `src/webhooks/delivery.rs` (mód tests já tem stub de Store e harness de
emit). Novo teste de isolamento: dois tenants, cada um com uma sub ativa
(clicked) apontando pra um mock server distinto; emitir um `link.clicked` com
`tenant_id = 1`; assertar que só a sub do tenant 1 recebe. E teste de gate:
uma sub clicked só no tenant 1 → `clicked_subscribed` vira true (hoje seria
false).

## Critérios de aceite

- [ ] `WebhookEvent` carrega `tenant_id`; todos os sites setam.
- [ ] Worker carrega subs de todos os tenants; atomics = any-tenant.
- [ ] `link.clicked`/`link.expired` de tenant != 0 disparam para as subs do
      próprio tenant, e nunca para as de outro tenant.
- [ ] Fail-open preservado; OSS single-tenant inalterado.
- [ ] Suíte completa + clippy + fmt verdes.
