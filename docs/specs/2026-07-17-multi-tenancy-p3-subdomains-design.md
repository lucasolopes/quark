# P3-completion — Subdomínio automático por tenant + Task 5 (create-flow) (cloud-only)

**Status:** design (decisão do host compartilhado tomada = subdomínio-auto). Fecha o P3 (LUC-8): destrava a T5 e o P3-frontend. Base `main @ 80ffccb`. Cloud-only.

## Objetivo

Todo tenant cloud ganha automaticamente `<slug>.<suffix>` (ex.: `acme.quarkus.com.br`) como domínio verificado, servindo seus links de cara — sem configurar domínio próprio e **sem furar o FORCE RLS de `links`**. Domínio 100% próprio (custom) continua opcional (P3-backend já entregue). Resolve o LUC-13 no front.

## Decisão de design (chave): subdomínio = linha materializada em `domains`

Em vez de um branch de parsing de subdomínio no `HostRouter` (que exigiria `get_tenant_by_slug` + um id de namespace de alias sem colisão — as sequences `quark_tenant_id_seq` e `quark_domain_id_seq` são independentes e ambas começam em 1, então reusar `tenant_id` como `domain_id` colidiria), **materializamos uma linha normal na tabela `domains`** por tenant:
- `host = <slug>.<suffix>`, `status = Verified`, `tenant_id = <tenant>`, `id = nextval(quark_domain_id_seq)` (mesma alocação de qualquer domínio custom).

Consequência: **zero special-casing.** `HostRouter.resolve()` → `get_domain_by_host("<slug>.<suffix>")` já acha (é um domínio verificado normal); o namespace de alias `(domain_id, alias)`, o filtro de isolamento `owned_by` no `Cache::get`, o `serve_wellknown` por Host e o `is_blocked_target` (SSRF) já funcionam sem tocar em nada. Só precisamos **semear** a linha.

## Contexto (do mapa)

- `HostRouter.resolve` (`src/domain_router.rs:74`) já resolve qualquer domínio verificado via `get_domain_by_host`; `public_host` (apex compartilhado) → tenant 0.
- `domains` (`src/store/postgres.rs`): `next_domain_id()` = `nextval(quark_domain_id_seq START 1)`; `SHARED_DOMAIN_ID=0`. `tenants.slug` é UNIQUE.
- Isolamento: `owned_by(rec, tenant)` (`src/cache/mod.rs:45`) em todo retorno do `Cache::get` — genérico sobre `route.tenant_id`, sem depender de como a rota foi derivada. **Sem gap pra subdomínio.**
- **Bug da T5:** `create()` (`POST /`, `src/api.rs:529`) chama `require_admin_for_create` que descarta o `Principal` (`src/api.rs:346`) e carimba `DEFAULT_TENANT` fixo (`:595`). `create_link_core` já é tenant-paramétrico (`tenant: TenantId`, `:380`) e carimba `Record.tenant_id`; só o alias hardcoda `SHARED_DOMAIN_ID` (`:453`). `admin_import` já faz certo (usa `p.tenant`).
- Frontend `shortUrl` (`web/src/components/LinkTable.tsx:27-33`) usa `PUBLIC_BASE` estático. `/admin/me` já expõe `memberships[].slug` (`src/api.rs:1740`, `web/src/lib/types.ts:68`).

## Arquitetura

### 1. Config + seeding do subdomínio
- Nova env `QUARK_TENANT_DOMAIN_SUFFIX` (ex.: `quarkus.com.br`). Cloud-only; se ausente, subdomínio-auto desligado (nenhuma linha semeada). Fica em `AppState`.
- **Na criação do tenant** (`admin_tenants_create`, `src/api.rs:~1837`): após `put_tenant`, se `multi_tenant` e o suffix estiver setado, criar um `domains` row `{ id: next_domain_id(), tenant_id, host: format!("{slug}.{suffix}").to_ascii_lowercase(), token: "" (n/a), status: Verified, created: now, verified_at: Some(now) }`. Idempotente (se já existe host, ignora — `put_domain` já mapeia UNIQUE→conflito).
- **Backfill** (migração no boot, `init_schema` ou um passo one-shot): pra cada tenant existente sem subdomínio, semear a linha. Como precisa do suffix (runtime env, não no schema SQL), o backfill roda no startup do app (não no DDL): após `init_schema`, se cloud+suffix, iterar tenants e garantir o `domains` row. Idempotente. (Alternativa: semear lazy no primeiro `/admin/me`/login — mas o boot-backfill é mais previsível.)

### 2. Namespace de alias default por tenant (T5)
Pra `<slug>.quarkus.com.br/promo` resolver, o alias precisa viver no `domain_id` do subdomínio do tenant, não em 0. Então:
- Helper `default_domain_id(tenant) -> u64`: no cloud, o id do `domains` row do subdomínio do tenant (via `get_domain_by_host` ou um lookup por tenant); senão `SHARED_DOMAIN_ID (0)`.
- `create_link_core` ganha o `domain_id` do alias vindo do default do tenant (substitui o `SHARED_DOMAIN_ID` hardcoded em `:453`). Numeric code continua global (resolve no subdomínio via `owned_by`).
- (Seleção explícita de domínio no create — escolher entre subdomínio / custom / shared — fica como refinamento futuro; v1 usa o subdomínio do tenant como default.)

### 3. Create-flow fix (o bug da T5)
- `require_admin_for_create` (`src/api.rs:346`) passa a retornar `Result<Principal, StatusCode>` (não descarta).
- `create()` (`:595`) carimba `p.tenant` (não `DEFAULT_TENANT`) e passa `default_domain_id(p.tenant)` como o domain do alias. OSS: `p.tenant` = tenant 0, domain 0 (idêntico a hoje).

### 4. Frontend (LUC-13)
- `shortUrl(code)` vira tenant-aware: do `useMe()`, pega o slug da membership atual (`memberships.find(current_tenant).slug`); se cloud+slug, monta `https://<slug>.<suffix>/<code>` (o suffix vem de `VITE_TENANT_DOMAIN_SUFFIX` ou de um campo no `/admin/me`); senão cai no `PUBLIC_BASE` (OSS/sem slug). Expor o suffix: adicionar ao `/admin/me` (ex.: `tenant_domain_suffix`) pra o front não precisar de env próprio.

### 5. wellknown / SSRF / isolamento
Sem mudança — cobertos pela linha `domains` materializada (é um domínio verificado normal).

## Escopo
**Dentro:** config `QUARK_TENANT_DOMAIN_SUFFIX`; seed na criação do tenant + backfill no boot; `default_domain_id` + create-flow fix (carimba `p.tenant` + alias no domínio default); `/admin/me` expõe o suffix; frontend `shortUrl` por subdomínio. Cloud-only; OSS byte-a-byte.
**Fora:** seleção explícita de domínio no create (UI picker) — futuro; emissão de TLS/DNS wildcard (infra do usuário, documentar); mudança de slug de tenant (assume-se imutável; se mudar, re-semear é follow-up).

## Testes
- **Isolamento subdomínio (chave):** link do tenant A servido em `a.<suffix>` → 302; mesmo código em `b.<suffix>` → 404. Alias `promo` do tenant A resolve em `a.<suffix>/promo`.
- **Seed:** criar tenant no cloud semeia o `domains` row Verified `<slug>.<suffix>`; backfill semeia os existentes; idempotente (boot 2x não duplica).
- **Create-flow fix:** link criado por um Principal cloud é carimbado com o tenant dele (não 0); alias vai no domínio default do tenant; OSS inalterado (tenant 0, domain 0).
- **Paridade OSS / suffix ausente:** sem `QUARK_TENANT_DOMAIN_SUFFIX` ou flag off → nenhum seed, create carimba como hoje, redirect idêntico.
- Postgres gated não-superuser; `-j1`; SEM `CREATE INDEX CONCURRENTLY`. Frontend Vitest.

## Riscos
1. **Bug do create carimbando tenant 0** já existe hoje (cloud links iam pro tenant 0) — o fix é pré-requisito e muda comportamento; testar OSS byte-a-byte + cloud carimba certo.
2. **Backfill no boot** precisa ser idempotente e barato (poucos tenants); rodar sob o advisory lock ou tolerar corrida entre réplicas (ON CONFLICT no host UNIQUE cobre).
3. **Colisão de id** evitada por materializar via `next_domain_id()` (mesma sequence dos customs) em vez de reusar `tenant_id`.
4. **Infra wildcard** é pré-requisito operacional (sem código): `*.<suffix>` DNS + TLS + apex roteando pro app de redirect. Documentar; sem isso o subdomínio não sobe.
5. **Slug mutável:** se um tenant trocar de slug, o `domains` row fica no host antigo. Assume imutável em v1; re-semear/renomear é follow-up.
