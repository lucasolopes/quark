# LUC-65 — GDPR fase 2: retenção + erasure + anonimização de IP

Data: 2026-07-19
Issue: LUC-65 (fase 2 do LUC-44). Base: `docs/research/2026-07-18-luc44-gdpr-consent.md`.

## Decisões do dono (2026-07-19)

1. **Retenção:** OSS/self-host = **ilimitado** por default (sem purge); cloud
   (`multi_tenant`) = **365 dias** por default; ambos **configuráveis** por env.
2. **Erasure:** só **endpoint de API** (sem UI nesta fase).
3. **Forwarding Meta:** flag **opt-in** que **anonimiza o IP** antes de enviar;
   default = comportamento atual.

## Storage (investigado)

- `click_events(seq, id, ts, referer, country, user_agent, city, variant,
  event_id, tenant_id)` — detalhe por clique (o dado quase-PII); tem `ts`. Alvo
  da retenção.
- `click_counters(id, dimension, bucket, count, tenant_id)` e
  `stats_meta(id, first_ts, last_ts, tenant_id)` — agregados por link. Alvo do
  erasure (apagar tudo de um link).
- Padrão de GC: `gc_sessions(now)` (`store` trait) chamado numa task horária em
  `main.rs:559`. Espelhar.
- `pixel::forward` (`src/pixel.rs:170`) insere `client_ip_address = e.ip` cru.

## Design

### Parte A — Retenção (purge de `click_events` por ts)
- Novo método `Store::purge_click_events_before(cutoff_ts: u64) -> Result<u64, StoreError>`
  (retorna nº apagado). **Postgres:** `DELETE FROM click_events WHERE ts < $cutoff`
  (global; retenção vale pra todos os tenants — a query não precisa de tenant
  scope). **LMDB:** o buffer de eventos já é limitado (`EVENTS_MAX=1000/link`,
  ring) — retenção por contagem já existe; implementar como no-op documentado
  OU podar por ts se for barato (decisão do implementer; o alvo real é o
  Postgres, unbounded). NÃO tocar `click_counters`/`stats_meta` (agregados, não
  são o dado de detalhe).
- Config em `main.rs`: `retention_secs` =
  - `QUARK_ANALYTICS_RETENTION_DAYS` (env) → `dias*86400` se setado;
  - senão: `multi_tenant ? Some(365*86400) : None`.
  - `None` = ilimitado → NÃO spawna a task de purge.
- Task periódica (molde do `gc_sessions`, ~1x/hora): quando `retention_secs =
  Some(r)`, chama `purge_click_events_before(now - r)`; loga o nº apagado;
  fail-open (erro só loga).

### Parte B — Erasure por link
- Novo método `Store::delete_link_analytics(tenant, id) -> Result<(), StoreError>`:
  numa transação (Postgres `begin_tenant_tx`), `DELETE FROM click_events`,
  `click_counters`, `stats_meta` WHERE `id=? AND tenant_id=?`. LMDB: apagar os
  buckets/eventos/stats do id (as chaves do id sob o prefixo do tenant/DEFAULT).
- Endpoint `DELETE /admin/links/:code/analytics` (`src/api.rs`, Scope::LinksWrite):
  resolve code→id, `delete_link_analytics(p.tenant, id)`, 204. (Não apaga o
  link em si — só os analytics dele.)

### Parte C — Anonimização de IP no forward Meta
- Env `QUARK_PIXEL_ANONYMIZE_IP` (bool opt-in). Threadar até `pixel::forward`
  (via um campo em `PixelBases` OU um parâmetro; escolher o mais limpo — o
  worker já monta `PixelBases`).
- Quando ligado, antes de inserir `client_ip_address`: **anonimizar** —
  IPv4 → zerar o último octeto (`a.b.c.0`); IPv6 → zerar os últimos 80 bits
  (manter /48). Se o IP não parsear, omitir a chave. Default (flag off) =
  comportamento atual (IP cru).
- (UA fica como está nesta fase; a decisão foi anonimizar IP.)

## Testes (TDD)
- Retenção: `purge_click_events_before` apaga só os eventos com `ts < cutoff`
  e preserva os novos (LMDB via open_backends se aplicável; Postgres gated em
  `QUARK_TEST_DATABASE_URL`).
- Erasure: `delete_link_analytics` remove events+counters+stats do link e
  NÃO toca outro link/tenant (isolamento). Endpoint: 204 + auth.
- Anonimização: uma função pura `anonymize_ip(&str) -> Option<String>`
  (IPv4 zera último octeto, IPv6 /48, inválido → None) com testes de mesa; e
  que `forward` com a flag ligada manda `a.b.c.0` (teste do payload, espelhando
  os testes de `pixel.rs`).

## Fora de escopo (documentar)
- UI de erasure no painel (decisão: só API).
- TTL nativo do ClickHouse (deferido junto com o provisionamento do ClickHouse,
  LUC-54); mencionar no PRIVACY.md/nota que quando o ClickHouse subir, a
  retenção lá é via TTL da tabela.
- Anonimização/omissão de UA (fase futura).
- Consentimento explícito do Meta além do GPC (o GPC do LUC-44 já corta o
  forward inteiro quando sinalizado).

## Docs
- `docs/PRIVACY.md` (+PT_BR): seção de retenção (defaults por modo + env),
  erasure (endpoint), e a nota do IP anonimizável no forward. Sem em-dash.

## Critérios de aceite
- [ ] Retenção configurável; default OSS ilimitado / cloud 365d; purge só de
      `click_events`, task horária, fail-open, sem purge quando ilimitado.
- [ ] `DELETE /admin/links/:code/analytics` apaga events+counters+stats do link
      (tenant-scoped), sem tocar outros.
- [ ] `QUARK_PIXEL_ANONYMIZE_IP` opt-in anonimiza o IP no forward (IPv4 último
      octeto / IPv6 /48); default inalterado.
- [ ] Testes acima + docs; suíte + clippy + fmt verdes.
