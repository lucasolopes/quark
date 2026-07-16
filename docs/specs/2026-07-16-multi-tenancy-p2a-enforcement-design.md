# P2a — Enforcement de tenant (FORCE RLS) + carry-overs

**Status:** design aprovado (aguardando revisão do spec antes do plano)
**Data:** 2026-07-16
**Sub-fase:** P2a de P2 (P2b ciclo de tenant/workspace · P2c convites · P2d OIDC por tenant). Depende de P1a+P1b (mergeados). Base `main @ 9890167`.

## Objetivo

Tornar um tenant ≠ 0 **realmente seguro** no modo cloud, ligando a rede de segurança fail-closed no nível do banco (`FORCE ROW LEVEL SECURITY` + `SET LOCAL app.tenant_id` por transação) e fechando os últimos pontos onde o código ainda fixa `DEFAULT_TENANT` mesmo com a auth já tenant-aware. É **backend puro, sem UX** — não cria tenant nem signup (P2b). O OSS (single-tenant) não muda: nada de FORCE, comportamento idêntico ao de hoje.

## Contexto (estado atual)

- As policies RLS já estão **definidas** em todas as 13 tabelas de `TENANT_OWNED_TABLES` desde o P1a (`src/store/postgres.rs:520-538`): `ENABLE ROW LEVEL SECURITY` + `USING (tenant_id = current_setting('app.tenant_id', true)::bigint)`. **Mas sem `FORCE`**, então o role dono (que roda o `init_schema` e serve requests) **ignora** as policies — o isolamento enforçado hoje é o `WHERE tenant_id` app-level.
- `begin_tenant_tx` (`src/store/postgres.rs:578-589`) já existe (`#[allow(dead_code)]`): abre uma tx no pool de escrita e faz `SET LOCAL app.tenant_id` via `set_config(..., true)`. Nunca é chamado.
- 4 pontos ainda presos no `DEFAULT_TENANT` (achados nos reviews do P1b, todos latentes hoje porque só existe o tenant 0): `create_link_core`/`admin_import`, `resolve_for_admin`, `resolve_code`, o state do OAuth do Sheets, e o snapshot do relay de webhooks.
- Não existe flag de modo OSS↔cloud (adiada do P1b).

## Decisões de arquitetura

### Flag de modo `QUARK_MULTI_TENANT`

Um env var lido no boot pra `AppState.multi_tenant: bool` (e passado ao `PostgresStore`). Ausente/`0` = OSS (default): comportamento de hoje, tudo tenant 0, **sem FORCE, sem tenant-tx**. `1` = cloud: enforcement ligado. É o primeiro consumidor real do flag; P2b/P3 penduram mais comportamento nele.

### FORCE RLS + transação por query (o ponto delicado)

No modo cloud, o `init_schema` emite `ALTER TABLE {t} FORCE ROW LEVEL SECURITY` nas tabelas tenant-owned (idempotente; no OSS **não** emite). Com `FORCE`, até o dono obedece a policy — então **toda query tenant-owned (leitura E escrita) precisa rodar numa transação que fez `SET LOCAL app.tenant_id` antes**, senão `current_setting` volta NULL e a policy filtra tudo (fail-closed → 0 linhas).

Mecanismo:
- `begin_tenant_tx` (escrita) já existe. Adicionar `begin_tenant_tx_read(tenant)` — igual, mas no pool de **leitura** (`self.read`), pra não perder o read/write split (uma tx só-SELECT numa réplica é válida). No caso single-URL (read=write, comum no P2a inicial) as duas viram o mesmo handle.
- Cada método tenant-owned do `PostgresStore` passa a: **no modo cloud**, rodar sua query dentro de `begin_tenant_tx`/`begin_tenant_tx_read` (`SET LOCAL` → query → commit); **no modo OSS**, rodar direto no pool como hoje (sem tx, sem overhead). Um helper interno encapsula o "escolha pool/tx conforme o modo" pra não duplicar em ~40 métodos.
- Métodos globais/infra (leases, `gc_sessions`, outbox claim/mark, lookups por hash) **não** entram em tenant-tx — não são tenant-scoped e as tabelas deles ou não têm RLS forçado ou o lookup é por chave única global. (Sessões/tokens: a tabela tem RLS, mas o lookup por hash precisa achar a linha sem saber o tenant antes — ver "Cuidado" abaixo.)

**Tradeoff (aceito pelo usuário):** no modo cloud, uma transação por query, inclusive reads. Mitigado: o hot path (redirect) é servido do cache L1 (moka), então o DB só é tocado em cache-miss; reads de admin/listagem aguentam o overhead. É o preço do isolamento fail-closed no banco.

**Cuidado — lookups por hash sob FORCE:** `get_api_token_by_hash`/`get_session_by_hash` acham a linha pelo hash **sem** saber o tenant (o tenant vem NA linha). Com FORCE numa tabela que tem `tenant_id`, um SELECT sem `app.tenant_id` setado volta 0 linhas → o lookup quebra. Solução: essas duas tabelas (`api_tokens`, `sessions`) **não recebem `FORCE`** (mantêm `ENABLE` sem `FORCE`), OU o lookup por hash roda como bypass. Decisão: **não forçar RLS em `api_tokens` e `sessions`** — o hash é aleatório e globalmente único (sem risco de vazamento cross-tenant por hash-guessing), e o tenant já viaja na linha e é validado pelo `admin_guard`. As outras 11 tabelas recebem FORCE. Documentar essa exceção no código.

### Fechar os 4 carry-overs

- **`create_link_core`** (`src/api.rs:361`) ganha um param `tenant: TenantId`; `admin_import` passa `p.tenant`; o `POST /` público passa o tenant resolvido (hoje `DEFAULT_TENANT` — no P2a segue `DEFAULT_TENANT` no OSS; a resolução por Host é P3).
- **`resolve_for_admin`** (`src/api.rs:~2147`) e **`resolve_code`** (`src/api.rs:~763`) ganham param de tenant; os handlers admin passam `p.tenant`. `resolve_code` é compartilhado com o redirect público → assinatura com tenant, o público passa `DEFAULT_TENANT` (P3 resolve por Host).
- **OAuth do Sheets:** `sheets_connect` põe o `p.tenant` no state assinado; `sheets_callback` lê o tenant do state e grava `put_sheets_connection(tenant, ...)` (hoje `DEFAULT_TENANT`, `src/api.rs:1738`).
- **Relay de webhooks:** `refresh_relay_snapshot` (`src/webhooks/delivery.rs:459`) hoje lista só `DEFAULT_TENANT`. No modo cloud precisa das assinaturas de **todos** os tenants (o outbox é cluster-wide). Como `list_webhooks` é tenant-scoped, adicionar um caminho não-escopado pro relay (ex. `list_all_webhooks()` global, ou o relay lê a assinatura por `subscription_id` da própria linha do outbox, que já carrega o tenant). Decisão: o relay resolve a assinatura pelo `subscription_id`+`tenant_id` da linha do outbox (a `OutboxDelivery` carrega o tenant), evitando um scan global.

## Escopo

**Dentro:** flag `QUARK_MULTI_TENANT`; `FORCE RLS` (11 tabelas) + `begin_tenant_tx`/`_read` roteando todo método tenant-owned no modo cloud; exceção documentada de `api_tokens`/`sessions`; os 4 carry-overs; testes.

**Fora:** criar tenant/signup/switcher (P2b); convites (P2c); OIDC por tenant (P2d); resolução Host→tenant (P3); billing.

## Testes

- **Fail-closed do RLS (o teste-chave, gated PG, modo cloud):** com `FORCE` ligado e `app.tenant_id` **não setado**, uma query numa tabela tenant-owned volta **0 linhas** (não erro). Com `SET LOCAL` do tenant certo, volta as linhas do tenant. E — a prova da rede de segurança — uma query que "esquece" o `WHERE tenant_id` (simular) ainda só vê as linhas do tenant setado.
- **Isolamento cross-tenant no modo cloud:** dois tenants, cada um só enxerga o seu, com o enforcement via tenant-tx (não só o `WHERE` app-level).
- **Paridade OSS:** com `QUARK_MULTI_TENANT` off, a suíte atual passa idêntica (sem FORCE, sem tenant-tx, tenant 0). O `tenant_isolation` existente continua verde.
- **Lookups por hash:** `get_api_token_by_hash`/`get_session_by_hash` funcionam no modo cloud (tabelas sem FORCE) — token/sessão criados são achados.
- **Carry-overs:** `admin_import` escreve no `p.tenant`; `resolve_for_admin`/`resolve_code` resolvem no tenant passado; Sheets OAuth grava no tenant do state; relay entrega pra assinatura do tenant certo.
- Gate `-j1`/`CARGO_BUILD_JOBS=1`; PG gated por `QUARK_TEST_DATABASE_URL`. **Verificação obrigatória antes do merge:** rodar o arm gated contra Postgres real com `QUARK_MULTI_TENANT=1` (o FORCE só se exercita com PG vivo) + dry-run da migração FORCE sobre dump de prod.

## Riscos

1. **FORCE RLS quebra reads sob pool** — se um método tenant-owned esquecer de rodar na tenant-tx no modo cloud, volta 0 linhas (bug silencioso de "sumiu tudo"). Mitigação: o helper centraliza o roteamento; teste que cobre cada método no modo cloud.
2. **Lookups por hash** — cobertos pela exceção de não-FORCE em `api_tokens`/`sessions` (documentada + testada).
3. **Migração FORCE em prod** — `ALTER ... FORCE` é metadata-only (rápido, sem rewrite), idempotente; validar no dry-run sobre dump de prod. Só roda no modo cloud.
4. **Overhead de tx por query** — aceito; cache L1 absorve o hot path. Não medir agora; observar se virar problema.
5. **Relay de webhooks cross-tenant** — resolver a assinatura pelo tenant da linha do outbox (não scan global) pra não vazar entre tenants.
