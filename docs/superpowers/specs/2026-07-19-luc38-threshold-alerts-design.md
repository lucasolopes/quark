# LUC-38 — Alertas de limiar de clique + templates n8n/Zapier

Data: 2026-07-19
Issue: LUC-38. Decisão do dono: **contador compartilhado** (Valkey quando
presente, fallback em memória por réplica — exato em single-node).

## Escopo do v1

Backend + docs + teste. Regra por link (N cliques em janela de M segundos) que
emite `link.threshold_reached` pelo caminho de entrega de webhook existente,
contada por um contador de **janela fixa compartilhado**, disparando **uma vez
por janela**. **UI do painel pra configurar a regra fica como follow-up** (a
regra é setável via API; o AC permite "config separada"). Criar o follow-up no
Linear ao final.

## Design

### 1. Tipo + persistência da regra (`src/store/mod.rs` + backends)
```rust
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AlertRule { pub threshold: u32, pub window_secs: u64 }
```
Tabela/keyspace novo `alert_rules`, tenant-owned (entra em
`TENANT_OWNED_TABLES` no Postgres). Métodos no trait `Store`:
- `put_alert_rule(&self, tenant, link_id, &AlertRule)`
- `delete_alert_rule(&self, tenant, link_id)`
- `get_alert_rule(&self, tenant, link_id) -> Option<AlertRule>`
- `list_alert_rules(&self, tenant) -> Vec<(u64, AlertRule)>`
Impl em LMDB (novo db `alert_rules`, chave `tkey_id(tenant, id)`), Postgres
(tabela `alert_rules(tenant_id BIGINT, link_id BIGINT, threshold BIGINT,
window_secs BIGINT, PRIMARY KEY (tenant_id, link_id))` + `CREATE TABLE IF NOT
EXISTS` no init + entrada no TRUNCATE de teste), e os stubs de mock
(`domain_router.rs`, `webhooks/delivery.rs` tests) com `unimplemented!()` ou
retorno vazio conforme o padrão dos outros métodos novos.

### 2. Admin API (`src/api.rs`)
- `PUT /admin/links/:code/alert` (Scope::LinksWrite): body `{ threshold, window_secs }`;
  valida `threshold >= 1` e `window_secs >= 60` (floor coerente com os outros
  timers); resolve code→id (`resolve_for_admin`), `put_alert_rule`. 200.
- `DELETE /admin/links/:code/alert`: `delete_alert_rule`. 204.
- (opcional) incluir a `alert` no GET do link, se o shape de resposta comportar
  fácil; não obrigatório no v1.
Rotas registradas perto das de `/admin/links/:code`.

### 3. Novo EventType (`src/webhooks/mod.rs`)
`LinkThresholdReached` → `"link.threshold_reached"`: adicionar ao enum, a
`as_str`, a `from_wire`, e garantir que `matches()` deixa uma subscription
recebê-lo. Frontend: adicionar em `WEBHOOK_EVENTS` (`web/src/lib/types.ts`),
`EVENT_LABEL_KEY` (`Webhooks.tsx`) e i18n EN/PT_BR, pra ser selecionável na
criação de webhook.

### 4. Motor de limiar no worker de analytics (`src/analytics/mod.rs`)
`spawn_worker` ganha dois parâmetros novos:
- `webhooks: Arc<WebhookDispatcher>` (pra emitir o evento; mesmo `emit`
  best-effort in-memory que clicked/expired usam).
- `control: Option<redis::aio::MultiplexedConnection>` (o control-conn do
  Valkey; `None` → contagem em memória por réplica).

Snapshot de regras: como o de pixels — no tick de 5s, `list_tenants()` +
`list_alert_rules(t)` → `Vec<(TenantId, HashMap<link_id, AlertRule>)>`,
fail-open (erro/timeout mantém o snapshot anterior).

No flush de cada batch, para cada `ClickEvent` cujo `(tenant, id)` tem regra:
- janela fixa `window = ev.ts / rule.window_secs`.
- **contador compartilhado** (espelha `abuse::ratelimit`): chave
  `quark:alert:cnt:{tenant}:{id}:{window}` — Valkey `INCR` + `EXPIRE
  window_secs*2`; sem Valkey, um mapa em memória `(tenant,id) -> (window,
  count)` com reset ao virar a janela (per-réplica).
- ao cruzar `count >= threshold`, disparar UMA vez por janela: marcador
  `quark:alert:fired:{tenant}:{id}:{window}` via `SET NX EX` (Valkey) / um
  `HashSet<(tenant,id,window)>` em memória. Se o marcador foi criado agora
  (não existia), `webhooks.emit(WebhookEvent { event_type:
  LinkThresholdReached, body, tenant_id })`.
- fail-open: erro de Valkey só loga, nunca derruba o worker nem o flush.

Payload: reusar o formato dos outros eventos. Como `webhook_event_payload`
vive em `api.rs`, expor um builder reutilizável (mover pra um lugar comum
tipo `webhooks::mod` OU criar `analytics`-local um `threshold_payload(code,
count, threshold, window_secs, ts)`); o body é JSON assinável (o `code` vem de
`codec::to_base62(permute::encode(id, key))` — o worker já tem `key`). Manter
o mesmo envelope (`id`, `type`, `data`) dos outros payloads.

### 5. main.rs
Passar `state.webhooks.clone()` e o control-conn (o mesmo usado pelo
ratelimiter/`control_conn`) pro `spawn_worker`.

### 6. Docs (`docs/WEBHOOKS.md` + `.PT_BR.md`)
Seção do evento `link.threshold_reached` (quando dispara, payload) + como
configurar a regra via API (`PUT /admin/links/:code/alert`), e **templates
prontos**: um fluxo n8n (nó Webhook → filtro/ação) e um Zapier (Catch Hook →
ação) consumindo o payload atual. Sem em-dash; EN/PT_BR sincronizados.

## Testes (TDD)
- Store: `put/get/list/delete_alert_rule` round-trip (LMDB via open_backends).
- Motor (unidade, sem Valkey → memória): alimentar N-1 cliques na janela não
  dispara; o N-ésimo dispara exatamente 1 evento; cliques extras na MESMA
  janela não re-disparam; ao virar a janela, um novo cruzamento dispara de
  novo. Injetar o dispatcher (canal) e assertar 1 `WebhookEvent`
  `link.threshold_reached` com o tenant certo.
- EventType round-trip (`as_str`/`from_wire`).

## Fora de escopo (follow-up)
- UI no painel pra configurar a regra (setável via API no v1). Criar issue.
- App nativo publicado Zapier/n8n (o issue já exclui).
- Precisão de janela deslizante (usamos janela fixa, como o rate limiter).

## Critérios de aceite
- [ ] Regra por link persistida (`alert_rules`), CRUD via API.
- [ ] Worker (fora do hot path) conta por janela compartilhada e emite
      `link.threshold_reached` uma vez por janela pelo caminho de entrega
      existente.
- [ ] Docs com templates n8n + Zapier.
- [ ] Testes do round-trip da store e da transição do limiar (não re-dispara
      na mesma janela; re-dispara na próxima).
- [ ] Suíte + clippy + fmt + web/tsc verdes.
