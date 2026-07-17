# P3 — Domínios próprios por tenant (cloud-only)

**Status:** design (aguardando revisão do spec antes do plano)
**Data:** 2026-07-16
**Fase:** P3 de multi-tenancy (LUC-8). Depende de P1a/P1b/P2a/P2b (mergeados; tenants reais + FORCE RLS ligados no cloud). Base `main @ fa9bf19`. Cloud-only (LMDB é OSS single-tenant; a feature só existe com `QUARK_MULTI_TENANT=1`).

## Objetivo

Cada tenant pluga o próprio domínio pro link curto (ex.: `go.acme.com`) em vez de só o host padrão compartilhado. Resolve também o LUC-13 (o painel copia o link com o host do backend). O namespace de short-code numérico continua global (Feistel/`permute`/`codec` intocados); o isolamento vem de amarrar cada domínio a um tenant e filtrar na resolução.

## Contexto (estado atual, do mapa de código)

- **Redirect** (`src/api.rs:1058` `redirect`, `:983` `unlock`): resolve `resolve_code(&st, DEFAULT_TENANT, &code)` (`:773`) — código numérico auto-decodifica (global, via `st.key`, zero DB); senão `get_alias(tenant, code)`. O header `Host` chega mas **não** é lido pra roteamento.
- **Aliases**: `aliases(alias TEXT PRIMARY KEY, id BIGINT, tenant_id BIGINT DEFAULT 0)` (`src/store/postgres.rs:440`); PK ainda é só `(alias)` → **globalmente único hoje**, mesmo com `tenant_id` na linha. Insert `ON CONFLICT (alias)` (`:826`,`:865`).
- **wellknown público** (`serve_wellknown`, `src/api.rs:2701`): serve `get_wellknown(DEFAULT_TENANT, name)` — tenant fixo, Host ignorado. Tabela `wellknown_documents` já tem PK `(tenant_id, name)` (template de migração).
- **Anti-SSRF/self-loop**: `is_blocked_target(host, headers, st)` (`src/api.rs:735`) usa `is_internal_host` (deny outbound, `src/abuse/mod.rs:14`) + `st.public_host: Option<String>` (`src/api.rs:42`) — um único self-host. Não existe allowlist de hosts.
- **Cache** (`src/cache/mod.rs`): `Cache` é tipado pra `Record` por `u64 id`, com Moka L1 (TTL 60s) + `CacheTier` L2 opcional (Valkey, TTL 3600s) + `Breaker` (timeout 100ms — invariante sagrada: L2 nunca bloqueia o redirect) + `Invalidator` pub/sub cross-replica. Miss hardcoda `get_link(DEFAULT_TENANT, id)`. **Não dá pra reusar direto** (é Record-shaped); o padrão sim.
- **init_schema** (`src/store/postgres.rs:419`): advisory-lock, `CREATE TABLE IF NOT EXISTS`, `TENANT_OWNED_TABLES` (`:88`) dá `tenant_id`+RLS ENABLE+POLICY, e (cloud) FORCE exceto `NOT_FORCED` (`api_tokens`/`sessions`/analytics — lookups no pool pelado antes do tenant). PK-rework via `DO $$ ... $$` (`:532`).
- **Frontend LUC-13**: `web/src/components/LinkTable.tsx:27` `shortUrl(code)` usa `VITE_API_BASE_URL || window.location.origin` — baked no host do backend (`fly.toml` `QUARK_PUBLIC_HOST=backend.quarkus.com.br`). Também `Shell.tsx:55` (display). `web/src/lib/api.ts:18` `BASE` é pra chamada de API (não mexer).
- **Flag**: `QUARK_MULTI_TENANT` → `AppState.multi_tenant` (gating de handler, ver `:1765`/`:1820`) + `PostgresStore.multi_tenant` (RLS/tx-shape).

## Decisões travadas (usuário, 2026-07-16)

1. **Verificação de posse = TXT + CNAME.** O tenant cria (a) um **TXT** `_quark-verify.<host>` com um token gerado pelo quark (prova de posse explícita, cobre até apex) e (b) aponta `<host>` pro quark via **CNAME** (roteamento; Fly/proxy emite o TLS). O domínio só serve tráfego depois de `verified`.
2. **Namespace de alias por domínio.** Dois tenants podem repetir o mesmo slug em domínios diferentes. A PK de `aliases` passa de `(alias)` pra `(domain_id, alias)`. O **host compartilhado é `domain_id = 0`** (sentinela, análogo a `DEFAULT_TENANT`), mantendo unicidade global lá; os aliases existentes migram pra `domain_id 0`.

## Arquitetura

### Modelo de dados

Tabela nova `domains` (cloud-only, entra em `TENANT_OWNED_TABLES` **e** em `NOT_FORCED` — o redirect a consulta por Host antes de saber o tenant, no pool pelado):

```
domains(
  id           BIGINT PRIMARY KEY,          -- nextval de quark_domain_id_seq (START 1; 0 = host compartilhado)
  tenant_id    BIGINT NOT NULL,             -- dono
  host         TEXT NOT NULL UNIQUE,         -- ex.: go.acme.com (único GLOBAL entre tenants)
  token        TEXT NOT NULL,               -- token de verificação (conteúdo do TXT)
  status       TEXT NOT NULL,               -- 'pending' | 'verified'
  created      BIGINT NOT NULL,
  verified_at  BIGINT
)
```
Índice `domains_by_tenant_id`. `host` único global impede dois tenants cadastrarem o mesmo host.

`aliases` re-chaveada: adiciona `domain_id BIGINT NOT NULL DEFAULT 0`; PK `(alias)` → `(domain_id, alias)` (migração `DO $$ ... $$` idêntica ao template de `wellknown_documents`, sem `CONCURRENTLY`); `ON CONFLICT (alias)` → `ON CONFLICT (domain_id, alias)`. Aliases existentes ficam em `domain_id 0` (host compartilhado). `tenant_id` continua na linha (RLS).

### `domain_id 0` = host compartilhado (sentinela)

Não é linha em `domains`. Representa o host padrão (`st.public_host`). Aliases no host compartilhado vivem em `domain_id 0` (namespace global, como hoje). Todo tenant pode criar link no host compartilhado.

### Resolução `Host → tenant` (hot path)

`redirect`/`unlock` leem `headers.get(header::HOST)` (lowercase, sem porta) e resolvem via um **`HostRouter`** novo (`src/domain_router.rs` ou similar), copiando o padrão do `Cache`: Moka L1 `host → Option<DomainRoute>` (TTL ~300s, domínios mudam raro) + L2 opcional (mesma trait `CacheTier`-style) + `Breaker` (timeout curto) + invalidação via o `Invalidator` (pub/sub) no add/remove/verify de domínio. `DomainRoute { domain_id, tenant_id }`.

- Host == `st.public_host` (compartilhado) → rota `{domain_id: 0, tenant: <nenhum específico>}` → resolução global (como hoje).
- Host é domínio custom `verified` → `{domain_id, tenant_id}`.
- Host desconhecido (ou `pending`) → **404** (não serve).

A `store.get_domain_by_host(host)` roda no pool pelado (por isso `domains` é `NOT_FORCED`).

### Isolamento na resolução

Curto e essencial: o code numérico continua global. Depois de `resolve_code` → `id` → `cache.get(id)` → `Record`:
- **Host custom** (`domain_id != 0`): exige `record.tenant_id == route.tenant_id`; senão **404** (sem vazamento cross-tenant). Aliases resolvem por `get_alias(domain_id, code)`.
- **Host compartilhado** (`domain_id 0`): serve global (qualquer tenant), como hoje. Aliases por `get_alias(0, code)`.

O `cache.get(id)` continua por id global (o `Record` carrega `tenant_id`); o filtro é no handler, barato, sem tocar a invariante do cache.

### Criação de link e escolha de domínio

`create_link_core` ganha um `domain_id` opcional (default `0` = host compartilhado). O alias é criado em `(domain_id, alias)`. Validação: o `domain_id` (se != 0) tem que ser um domínio `verified` do tenant do request (checar via `get_domain`/RLS). Código numérico não muda (global). O painel mostra um seletor de domínio no criar-link **só no cloud com ≥1 domínio verificado**.

### Endpoints admin (`/admin/domains`, cloud-only)

Gated `if !st.multi_tenant { 404 }` (padrão dos endpoints P2b). Todos tenant-scoped (RLS via `with_read!`/`with_write!`), pelo `Principal` do `admin_guard`:
- `GET /admin/domains` — lista os domínios do tenant (host, status, token, o registro DNS a criar).
- `POST /admin/domains {host}` — valida formato do host, rejeita host interno (`is_internal_host`) e o próprio `public_host`, cria `pending` com token gerado; retorna as instruções DNS (TXT `_quark-verify.<host>=<token>` + CNAME `<host> → <alvo do quark>`).
- `POST /admin/domains/:id/verify` — resolve o TXT via DNS; se bate o token → `verified` + invalida o `HostRouter`. Rate-limited (reusa o ratelimiter).
- `DELETE /admin/domains/:id` — remove; invalida o `HostRouter`.

Verificação DNS: resolver TXT de `_quark-verify.<host>` (lib de DNS async já usada em `src/health.rs`, ou `hickory-resolver`) e comparar com o token. A resolução DNS **não** roda no hot path — só no verify sob demanda.

### wellknown/AASA por Host

`serve_wellknown` resolve o tenant pelo Host (via `HostRouter`) em vez de `DEFAULT_TENANT`: host custom → AASA/assetlinks daquele tenant; host compartilhado → documento do tenant 0 (comportamento atual). Host desconhecido → 404. `WELLKNOWN_NAMES` (2 entradas) por tenant, como hoje.

### Anti-SSRF / self-loop

`is_blocked_target` passa a barrar redirect pra **qualquer** host do quark: o `public_host` **mais** todos os hosts `verified` em `domains`. Consulta via o `HostRouter` (o host de destino do link resolve pra uma rota do quark → é self-loop → barrado). Sem novo mecanismo de DNS; só amplia o conjunto de "hosts nossos".

### Frontend

- Tela **Domínios** (nova rota, cloud-only na sidebar): adicionar host, mostrar o registro TXT + o CNAME a criar, botão "verificar", status (pending/verified), remover. Padrão dos componentes existentes (Card/Dialog/Button/Table).
- `shortUrl(code)` no `LinkTable` vira **domain-aware**: usa o domínio custom `verified` "primário" do tenant (do `/admin/domains` ou de um campo já carregado) se houver, senão o host compartilhado (`PUBLIC_BASE`). Corrige o LUC-13. `LinkTable.test.tsx:66` atualiza junto.
- Seletor de domínio no criar-link (só cloud com ≥1 verificado).
- `MeResponse`/endpoint expõe os domínios verificados do tenant atual pro front montar a URL. TLS: dependência documentada (Fly/proxy).

## Escopo

**Dentro:** tabela `domains` + migração; `aliases` re-chaveada `(domain_id, alias)` + migração dos existentes pra `domain_id 0`; `HostRouter` (host→rota) com cache/breaker/invalidação; resolução por Host + isolamento no redirect/unlock; `create_link_core` com `domain_id`; endpoints `/admin/domains` (CRUD + verify DNS TXT); wellknown por Host; SSRF cobrindo todos os hosts; frontend (tela de domínios, `shortUrl` domain-aware, seletor no criar-link). Cloud-only.

**Fora:** emissão de TLS (Fly/proxy — só documentar); LMDB (OSS single-tenant, não ganha domínios); OIDC por tenant (P2d/LUC-25); convites (P2c). Código numérico continua global (Feistel/codec/permute intocados).

**Split provável de implementação** (como no P2): **P3-backend** (domains+migração+aliases-namespace+HostRouter+resolução/isolamento+create_link+endpoints+verify+wellknown+SSRF) e **P3-frontend** (tela de domínios + `shortUrl` + seletor). Decidido no plano.

## Testes / verificação

- **Isolamento (chave):** código numérico do tenant A servido em `go.acme.com` (tenant A) resolve; o MESMO código servido em `go.beta.com` (tenant B) → 404. Alias `promo` coexiste em dois domínios apontando pra links diferentes. Host compartilhado resolve global.
- **Verificação DNS:** TXT com o token → `verified`; TXT ausente/errado → continua `pending`, não serve. `DELETE`/re-add invalida a rota (host desconhecido → 404 na hora certa).
- **wellknown por Host:** AASA do tenant certo pelo host; host desconhecido → 404.
- **SSRF:** criar link apontando pra um host `verified` do quark → barrado (self-loop); host externo comum → ok.
- **Paridade OSS:** `QUARK_MULTI_TENANT` off → `/admin/domains` 404, sem tabela obrigatória de rota, redirect idêntico ao de hoje (host único), aliases global; suíte atual passa igual.
- **Migração:** dry-run sobre dump de prod — aliases viram `domain_id 0`, unicidade global preservada; `domains` criada vazia; RLS on.
- **Cache/breaker do HostRouter:** L2 lento não bloqueia o redirect (mesma invariante de 100ms).
- Postgres gated por `QUARK_TEST_DATABASE_URL`, verificação **como role NÃO-SUPERUSER** em modo cloud (RLS real; `domains` em NOT_FORCED confere que o lookup por Host funciona no pool pelado). Gate `-j1`. Frontend: Vitest. **SEM `CREATE INDEX CONCURRENTLY`.**

## Riscos

1. **Vazamento cross-tenant no redirect.** O código numérico é global; sem o check `record.tenant_id == route.tenant_id` num host custom, o link de um tenant apareceria no domínio de outro. Mitigação: o filtro pós-resolução é o coração da feature; teste de isolamento explícito (A em go.beta → 404).
2. **`domains` em NOT_FORCED** significa que o lookup por Host roda sem RLS — é de propósito (o tenant é desconhecido nesse ponto), mas o CRUD admin de domínios **tem** que rodar tenant-scoped (RLS via `with_read!`/`with_write!`) pra um tenant não ler/mexer domínio de outro. Mitigação: separar o caminho público (`get_domain_by_host`, pelado) do caminho admin (tenant-tx); testar como não-superuser que o admin não enxerga domínio de outro tenant.
3. **Re-chavear a PK de `aliases`** muda a garantia (global → por-domínio) e mexe numa tabela quente. Mitigação: migração idempotente com o template testado do `wellknown_documents`, `domain_id 0` preserva os existentes, dry-run sobre prod, sem `CONCURRENTLY`.
4. **Resolução DNS no verify** pode ser lenta/falhar — nunca no hot path (só sob demanda, rate-limited); timeout + erro amigável.
5. **Cache de rota stale** serviria um domínio removido/não-verificado. Mitigação: invalidação via `Invalidator` no add/remove/verify + TTL; host desconhecido/pending sempre 404.
6. **LUC-13 no front:** confundir a base de API (`api.ts` BASE, fica no backend) com a base de exibição (`shortUrl`, vira domain-aware). Mitigação: só `shortUrl`/display mudam; a chamada de API não.
