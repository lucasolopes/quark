# SSO discovery by email domain (Home Realm Discovery) — cloud-only (LUC-57)

**Status:** design (aguardando aprovação antes de implementar). Base `main @ 7912dc3`. Cloud-only, opt-in. Substitui a ideia de "painel por host" após pesquisa (Dub.co não faz; WorkOS/Auth0/Okta usam descoberta por e-mail).

## Objetivo

No login central (`app.quarkus.com.br`), o usuário de uma empresa com SSO digita o e-mail; se o domínio (`@acme.com`) pertence a um tenant com OIDC próprio, ele é roteado direto pro login daquele tenant (`/admin/login?org=<slug>`, que já existe) sem digitar slug e sem host por-tenant. Quem não bate cai no login compartilhado de sempre. É "Home Realm Discovery".

## Contexto (verificado no código)

- **Verificação por DNS TXT já existe (P3):** tabela `domains` (`id, tenant_id, host UNIQUE, token, status, created, verified_at`), o seam `Dns` (`src/dns.rs`, em `AppState.dns`), e o handler `admin_domains_verify` que faz o lookup TXT e vira `status=verified`. Reusamos o **padrão** (não a mesma tabela — ver Arquitetura).
- **Login por-tenant já existe (P2d/P2e):** `GET /admin/login?org=<slug>` resolve o tenant, pega o `oidc_config`, redireciona pro IdP; 404 genérico se slug desconhecido/sem-config (anti-enumeração). O `Login.tsx` já lê `?org=` (LUC-53).
- **`oidc_config` por-tenant** marca quais tenants têm SSO próprio (`get_oidc_config_bare`).
- **Rate-limiter** em `AppState.ratelimiter` (usado no create de tenant, tokens, etc.).

## Arquitetura

### 1. Modelo: domínios de e-mail de SSO por tenant (tabela dedicada)
Nova tabela `sso_email_domains` (NÃO reusar `domains`, cujo `host UNIQUE` é o namespace de short links e cujo lookup está no caminho quente do redirect — manter isolado por segurança):

```
sso_email_domains (
  id BIGINT PRIMARY KEY,
  tenant_id BIGINT NOT NULL,
  domain TEXT NOT NULL UNIQUE,     -- e-mail domain, lowercase (ex.: acme.com)
  token TEXT NOT NULL,             -- desafio TXT (_quark-sso.<domain> = token)
  status TEXT NOT NULL,            -- 'pending' | 'verified'
  created BIGINT NOT NULL,
  verified_at BIGINT
)
```
- `domain` é UNIQUE global: um domínio de e-mail pertence a no máximo um tenant (verificado). Impede dois tenants reivindicarem `acme.com`.
- Cloud-only + `TENANT_OWNED`/`NOT_FORCED` conforme o padrão das outras tabelas (lookup bare por domínio antes de saber o tenant → `NOT_FORCED`, isolamento app-level `WHERE tenant_id` nas escritas por-tenant).
- Store: `put_sso_domain`, `get_sso_domain_bare(domain)` (bare, pro discover), `list_sso_domains(tenant)`, `set_sso_domain_status(tenant, id, status, verified_at)`, `delete_sso_domain(tenant, id)`. Espelham as assinaturas de `domains`.

### 2. Verificação (reusa o seam `Dns`)
- Ao adicionar um domínio de SSO: gera `token`, grava `status=pending`. A UI mostra "crie o TXT `_quark-sso.acme.com = <token>`".
- `admin_sso_domains_verify(tenant, id)`: faz o TXT lookup via `st.dns` (igual `admin_domains_verify`), e se o token bate → `status=verified, verified_at=now`. Só um domínio **verified** conta na descoberta.
- Só permitido quando o tenant tem `oidc_config` (senão o SSO não existe pra rotear). Sem config → 400/409.

### 3. Endpoint de descoberta
`GET /admin/sso/discover?email=<email>` — **não autenticado**, rate-limited por IP:
- Extrai o domínio do e-mail (parse simples pós-`@`, lowercase; e-mail malformado → resposta "não encontrado").
- `get_sso_domain_bare(domain)`: se existe, `status=verified`, e o tenant ainda tem `oidc_config` → `200 { "org": "<slug>" }`. Senão → `200 { }` (vazio; NÃO 404, resposta uniforme).
- **Anti-enumeração:** resposta 200 nos dois casos (só muda o corpo), rate-limit por IP. HRD inerentemente revela se um domínio tem SSO — mitigar com rate-limit, não com sigilo total. Nunca vaza o tenant_id, só o slug (que já é usável em `?org=`).
- OSS (`!multi_tenant`) → 404 (o endpoint não existe fora do cloud).

### 4. Login com e-mail primeiro (`Login.tsx`)
- Estágio de e-mail: campo de e-mail + "Continuar". No submit chama `api.discoverSso(email)`.
  - Achou `org` → `window.location.href = oidcLoginUrl(org)` (SSO do tenant).
  - Não achou → cai no login compartilhado atual (botão do provedor global + campo de token), sem fricção extra.
- `?org=` na URL (deep-link, LUC-53) continua com precedência: se já veio org, pula o passo de e-mail e mostra "Entrar em `<slug>`".
- Sem `oidc_enabled` (OSS/sem OIDC) → comportamento atual intocado (o passo de e-mail nem aparece).
- `api.discoverSso(email)` novo em `api.ts`: `GET ${BASE}/admin/sso/discover?email=` → `{ org?: string }`.

### 5. UI de admin dos domínios de SSO (última task)
- Tela/aba em Settings: listar domínios de SSO, adicionar (mostra o registro TXT), botão verificar, remover. Só aparece pra tenant com `oidc_config`. Espelha a UI de domínios custom do P3 se existir; senão, um form simples.
- Endpoints admin: `GET/POST/DELETE /admin/sso-domains` + `POST /admin/sso-domains/:id/verify`, todos `admin_guard(Full)` + cloud-only + tenant-scoped.

## Escopo
**Dentro:** tabela + store dos domínios de SSO; verificação TXT (reusa `Dns`); endpoint de descoberta; login e-mail-primeiro no `Login.tsx`; CRUD admin + UI. Cloud-only, opt-in, reusa `?org=`/login por-tenant.
**Fora:** white-label de domínio do painel (`panel_domain` — só se virar exigência de venda); tema do Keycloak (config operacional do realm, documentar no runbook, não é código); mudar o login compartilhado ou o OSS.

## Testes
- **Store (PG gated):** put/get_bare/list/set_status/delete; `domain` UNIQUE (segundo tenant reivindicando o mesmo domínio → conflito); só verified conta.
- **Verificação:** TXT bate → verified; não bate → continua pending (mock `Dns`, igual aos testes de `domains`).
- **Descoberta:** domínio verified de tenant com oidc_config → `{org}`; domínio pending → vazio; domínio inexistente → vazio; tenant sem oidc_config → vazio; e-mail malformado → vazio; OSS → 404; rate-limit por IP dispara. Nunca vaza tenant_id.
- **Login (web/Vitest):** email → discover achou → redireciona pro `oidcLoginUrl(org)`; não achou → mostra login compartilhado; `?org=` presente pula o passo (regressão LUC-53); OSS/sem oidc → passo de e-mail ausente.
- **Admin CRUD:** cloud-only, admin_guard(Full), tenant-scoped; só com oidc_config.
- Paridade OSS: nada de SSO-domains fora do cloud; suíte atual verde. `-j1`, sem CONCURRENTLY, PG não-superuser.

## Riscos
1. **Hijack de domínio** (tenant reivindica `gmail.com`/domínio alheio) → verificação DNS TXT **obrigatória** + `domain UNIQUE`; só verified roteia.
2. **Enumeração** via discover → rate-limit por IP + resposta 200 uniforme; aceitar que HRD revela existência de SSO por design (padrão da indústria).
3. **Domínio compartilhado** (dois tenants, mesmo domínio de e-mail) → `UNIQUE` global resolve por "primeiro a verificar ganha"; documentar. (Caso raro; empresas com SSO usam domínio próprio.)
4. **Precedência no Login** entre `?org=` (deep-link) e o passo de e-mail → `?org=` vence e pula o e-mail; testar.
5. **`NOT_FORCED`** na tabela (lookup bare) → isolamento das escritas é app-level `WHERE tenant_id`; teste de isolamento cross-tenant.
