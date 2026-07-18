# LUC-36 — Forward de pixels por tenant (com isolamento cross-tenant)

Data: 2026-07-17
Issue: LUC-36 (Analytics particionado por tenant_id)
Relacionado: LUC-63 (mesmo padrão no worker de webhooks, adiado)

## Diagnóstico

A descrição do LUC-36 está desatualizada. O trabalho de particionamento de
analytics já foi feito no P4a, exceto pelo worker de pixels:

- ClickHouse: a tabela `clicks` já tem `tenant_id` (com `ORDER BY (tenant_id,
  id, ts)` e migração idempotente `ADD COLUMN`), o `INSERT` já grava
  `e.tenant_id`, e `stats_for_tenant` já filtra por `WHERE tenant_id = ?`.
- Postgres: `stats_for_tenant` e o índice `click_counters_by_tenant` já
  existem e filtram por tenant.

O único gap real está em `src/analytics/mod.rs`, no worker de forward de
pixels (conversão server-side), com dois defeitos:

1. `refresh_pixel_snapshot` lê só `store.list_pixels(DEFAULT_TENANT)`. Em modo
   cloud, pixels de outros tenants nunca carregam, então conversões desses
   tenants nunca são encaminhadas.
2. `forward_to_pixels` encaminha o batch inteiro para cada pixel ativo. O
   batch mistura cliques de vários tenants. Assim que (1) for corrigido, os
   cliques do tenant A seriam enviados ao pixel de conversão do tenant B —
   vazamento cross-tenant de dados de conversão.

O tipo `PixelConfig` não carrega `tenant_id`, então o pareamento
pixel↔tenant precisa viver no snapshot do worker.

## Escopo

Corrigir os dois defeitos juntos, tudo contido em `src/analytics/mod.rs`.
Sem mudança no trait `Store`, na assinatura de `spawn_worker`, de `flush`,
nem de `pixel::forward`.

Fora de escopo:
- Migração retroativa de dados históricos do tenant 0 (o issue já exclui; e
  não há prod com dado real — prod será resetado).
- O worker de webhooks (`src/webhooks/delivery.rs`), que tem o mesmo padrão
  `DEFAULT_TENANT`-only — rastreado em LUC-63.

## Design

### 1. Tipo do snapshot

De `Vec<PixelConfig>` para `Vec<(TenantId, Vec<PixelConfig>)>`: cada grupo de
pixels pareado com o tenant dono. Grupos sem pixels são omitidos.

### 2. `refresh_pixel_snapshot`

Passa a enumerar todos os tenants:

- `store.list_tenants()` e, para cada tenant, `store.list_pixels(t.id)`.
- Reusa o caminho `with_read!`/RLS por-tenant já existente e testado; zero
  mudança no trait `Store`. É N+1 queries, mas roda no tick de 5s, fora do
  hot path.
- Um único orçamento de `PIXEL_SNAPSHOT_TIMEOUT` (3s) envolve a enumeração
  inteira. Fail-open preservado: em erro (de `list_tenants` ou de qualquer
  `list_pixels`) ou timeout, o snapshot anterior é mantido intacto e a falha
  é apenas logada — um store travado nunca esvazia um snapshot que estava
  bom.
- OSS/single-tenant: `list_tenants()` devolve só o default, então o
  comportamento degrada exatamente para o de hoje.

### 3. `forward_to_pixels`

Para cada `(tenant, configs)` no snapshot:

- filtra `events` por `e.tenant_id == tenant.0` uma vez por tenant;
- se o subconjunto for vazio ou não houver pixel ativo, pula o tenant;
- caso contrário, encaminha só esse subconjunto a cada pixel ativo daquele
  tenant.

Isso fecha o vazamento cross-tenant. O fail-open por-provider (erro de um
provider só é logado, nunca propagado) é mantido.

## Fluxo de dados

```
worker tick (5s)
  └─ refresh_pixel_snapshot
       └─ list_tenants() → [t0, t1, ...]
            └─ para cada t: list_pixels(t) → snapshot: [(t, [pixels]), ...]
flush
  ├─ sink.record_batch(buf)            (já particionado por tenant_id)
  └─ forward_to_pixels(snapshot, buf)
       └─ para cada (t, pixels):
            scoped = buf.filter(ev.tenant_id == t)
            para cada pixel ativo: pixel::forward(scoped)
```

## Tratamento de erro

- `list_tenants`/`list_pixels` falha ou timeout: mantém snapshot anterior,
  loga (fail-open).
- Falha de forward de um provider: loga com `pixel_id`, segue para o próximo
  (comportamento atual, preservado).

## Testes (TDD)

Todos em `tests/pixel_forward_it.rs`, reusando o mock server que captura os
POSTs por rota.

- **Isolamento cross-tenant (novo, principal):** dois tenants, cada um com um
  pixel apontando para um mock distinto; um batch com eventos de ambos os
  tenants; asserta que cada mock recebe apenas os eventos do seu tenant.
  Prova simultaneamente que (a) tenant != 0 é encaminhado e (b) não há
  vazamento.
- **Helper:** novo `ev_t(id, ts, tenant)` (o `ev` atual fixa `tenant_id: 0`).
- **Regressão:** os 5 testes existentes continuam verdes (tenant 0 é o caso
  degenerado do novo caminho).

## Critérios de aceite

- [ ] Worker carrega pixels de todos os tenants (não só `DEFAULT_TENANT`).
- [ ] Eventos de um tenant nunca são encaminhados ao pixel de outro tenant.
- [ ] Fail-open preservado (store travado mantém snapshot; provider com erro
      não derruba o worker).
- [ ] OSS/single-tenant inalterado.
- [ ] `cargo test` verde, incluindo o novo teste de isolamento.
