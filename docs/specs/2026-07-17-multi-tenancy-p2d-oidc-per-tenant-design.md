# P2d — OIDC por-tenant (login por slug) (cloud-only)

**Status:** design (decisão do usuário: login por slug `/login?org=`). Fecha o P2 (LUC-7/LUC-25). Base `main @ 4f4e8d6`. Cloud-only; o OIDC global de env fica intocado (login de plataforma/OSS).

## Objetivo

Um tenant enterprise pluga o próprio IdP OIDC. O login resolve **qual IdP usar pelo slug na URL** (`/login?org=acme` → `/admin/login?org=acme`), antes de autenticar. A autorização no cloud continua vindo do papel da membership (P2b) — o claim de grupo do IdP do tenant mapeia pro `Role` da membership criada/atualizada no login. O OIDC global de env (`st.oidc`) permanece como login de plataforma (sem `?org`) e do OSS.

## Contexto (do mapa)

- `OidcConfig` (`src/oidc.rs:20-60`): `issuer, client_id, client_secret, redirect_url, scopes, admin_claim, admin_value, readonly_value, post_login_url`; `from_env()` (gate = `QUARK_OIDC_ISSUER`). Global em `AppState.oidc: Option<Arc<OidcRuntime>>` (`src/api.rs:48`) + `oidc_configured: bool`. `OidcRuntime` (`:380`) tem discovery + `RwLock<Jwks>` únicos (precisa virar cache por tenant/issuer).
- Login start `oidc_login` (`src/api.rs:1547`): só o `HeaderMap`, nenhum contexto de tenant hoje. Assina `state/verifier/nonce` no cookie `qk_login` via `sign_login_state` (`src/oidc.rs:457`).
- Callback `oidc_callback` (`src/api.rs:1590`): valida `qk_login`, troca code, valida id-token (JWKS por `kid`), `map_scopes`, `ensure_user_and_membership` (`src/oidc.rs:335`; cloud NÃO cria membership hoje), cria sessão (hardcoda `DEFAULT_TENANT`).
- `admin_guard` cloud (P2b): scopes vêm de `role_scopes(membership no session.tenant_id)` a cada request — `session.scopes` do claim é ignorado pós-login no cloud.
- Break-glass admin token: checado primeiro, sempre `Scope::Full` em tenant 0 — **intocável**.
- Precedente de secret at rest: `sheets_connection.blob` JSONB **plaintext** (refresh token). `oidc_configs` seguirá o mesmo (client_secret plaintext no blob) — decisão do usuário; hardening (cifra) = follow-up.
- `get_tenant_by_slug` NÃO existe (só `get_tenant(id)`); `tenants.slug` é UNIQUE. Precisa ser adicionado.

## Decisões travadas (usuário)

1. **Resolução de login por slug** (`?org=<slug>`). Menor diff, coexiste limpo com o env global, não depende do P3.
2. **`client_secret` plaintext** no blob (precedente Sheets); issue de hardening (cifra) separada.
3. **Coexistência:** OSS/plataforma = env global (`st.oidc`) intocado; cloud adiciona lookup por-tenant **além** do global.
4. **Claim → papel:** o admin-group-claim do IdP do tenant mapeia pro `Role` da membership (não pra `Scope` direto), espelhando o OSS; no cloud a autorização já é por papel da membership.

## Arquitetura

### Tabela `oidc_configs` (cloud-only; TENANT_OWNED + NOT_FORCED)
```
oidc_configs(
  id          BIGINT PRIMARY KEY,   -- nextval quark_oidc_config_id_seq
  tenant_id   BIGINT NOT NULL,      -- 1 config por tenant (UNIQUE tenant_id)
  issuer      TEXT NOT NULL,
  blob        JSONB NOT NULL,       -- { client_id, client_secret, scopes, admin_claim, admin_value, readonly_value, post_login_url? }
  created     BIGINT NOT NULL
)
```
`UNIQUE(tenant_id)` (1 IdP por tenant em v1). Índice por tenant. Vai em `TENANT_OWNED_TABLES` (RLS ENABLE) **e** `NOT_FORCED` (o login resolve por slug→tenant e lê a config no pool pelado, antes de haver `app.tenant_id`). Sequência + `reset_for_tests`.

### Tipo + store
`TenantOidcConfig { tenant_id, issuer, client_id, client_secret, scopes, admin_claim, admin_value, readonly_value, post_login_url }` (`src/oidc.rs` ou `src/tenant_oidc.rs`). Store: `put_oidc_config`, `get_oidc_config(tenant)` (tenant-scoped, pra o CRUD admin), `get_oidc_config_by_tenant_bare(tenant)` (bare, pro login/callback), `delete_oidc_config(tenant)`, `next_oidc_config_id`. `get_tenant_by_slug(slug)` (bare) pra resolver o `?org=`.

### CRUD admin (`/admin/oidc-config`, cloud-only, Owner/Admin)
- `PUT /admin/oidc-config {issuer, client_id, client_secret, scopes, admin_claim, admin_value, readonly_value, post_login_url?}` — `admin_guard(Scope::Full)`; upsert da config do tenant. Rate-limited.
- `GET /admin/oidc-config` — retorna a config **sem o client_secret** (ou com um placeholder), pra a UI.
- `DELETE /admin/oidc-config` — remove; o tenant volta a não ter OIDC próprio.

### `OidcRuntime` por tenant (cache)
`OidcRuntime` vira construível a partir de um `TenantOidcConfig` (não só do env). Um cache keyed por `tenant_id` (ou issuer) em `AppState` (ex.: `oidc_tenants: Moka<u64, Arc<OidcRuntime>>` ou um `RwLock<HashMap>`), com discovery + JWKS por entrada, refresh lazy no signature-mismatch (como hoje). TTL/eviction pra o tenant reconfigurar o IdP sem ficar preso em cache velho. Invalidação no `PUT`/`DELETE` da config.

### Login start (`/admin/login?org=<slug>`)
- Sem `?org` → comportamento atual (env global `st.oidc`; se não configurado, o de hoje).
- Com `?org=<slug>` → `get_tenant_by_slug(slug)` → `get_oidc_config_by_tenant_bare(tenant)`; monta/pega o `OidcRuntime` do tenant; `authorize_url`. **Assina o `tenant_id` no cookie `qk_login`** (estender `sign_login_state`/`verify_login_state` pra carregar um 4º campo). Slug/tenant sem config → erro claro (o tenant não tem OIDC próprio; cair no global? não — 404/mensagem).

### Callback (`/admin/callback`)
- Recupera `(state, verifier, nonce, tenant_id?)` do `qk_login`. Se tem `tenant_id` → usa o `OidcRuntime` daquele tenant pra validar (issuer/aud/JWKS da config do tenant); senão → o env global (como hoje).
- `ensure_user_and_membership`: upsert do `User` por `subject` (como hoje). **No cloud com tenant do login**: cria/atualiza a `Membership(user, tenant_do_login, role)` onde `role` vem do mapeamento do claim de grupo do IdP do tenant (`admin_value`→Admin/Owner? readonly→Viewer; default Member) — decisão: mapear pra `Role`, não `Scope`. Sessão `tenant_id = tenant_do_login`.
- Preserva o contrato de status; break-glass e API-token intocados.

### Frontend (P2d-frontend)
- `Login.tsx`: um campo/step de organização (slug) → botão "entrar com o provedor da sua org" → `oidcLoginUrl(orgSlug)` = `/admin/login?org=<slug>`. O botão do provedor global continua (sem org) pra plataforma. Um endpoint leve `GET /admin/oidc-config/exists?org=<slug>` (público, só diz se o tenant tem OIDC) pode gatear o botão — opcional v1 (pode só tentar e tratar erro).

## Escopo
**Dentro:** tabela `oidc_configs` + tipo + store + `get_tenant_by_slug`; CRUD admin; `OidcRuntime` por-tenant cacheado; login `?org=` + tenant no cookie; callback valida contra a config do tenant + cria membership com papel do claim; OSS/global intocado; frontend org-login. Cloud-only.
**Fora:** cifrar `client_secret` (follow-up); múltiplos IdPs por tenant (1 por tenant em v1); SSO discovery automático; SCIM/provisioning. Break-glass/API-token inalterados.

## Testes
- **Login por slug:** `?org=acme` monta o authorize URL com a config do tenant Acme; assina o tenant no cookie. Slug inexistente / sem config → erro (não cai no global silenciosamente).
- **Callback por tenant:** id-token validado contra o issuer/JWKS do tenant; cria membership no tenant do login com o papel do claim; sessão no tenant certo. Token de outro issuer → rejeitado.
- **Coexistência:** sem `?org` → env global (comportamento atual); OSS byte-a-byte (suíte OIDC atual passa igual).
- **Isolamento:** config de um tenant não vaza pro outro (CRUD tenant-scoped; `get_oidc_config` não expõe secret). Login no tenant A não dá acesso ao B.
- **Break-glass:** admin token continua Scope::Full tenant 0, independente de OIDC por-tenant.
- Postgres gated não-superuser; `-j1`; sem CONCURRENTLY. Auth = review adversarial no Opus. Frontend Vitest.

## Riscos
1. **Auth é bug-prone** (o OIDC anterior levou 20 achados). Mitigação: tasks pequenas, review Opus, preservar contrato de status + break-glass byte-a-byte; teste de cada caminho.
2. **`client_secret` plaintext** — decisão consciente (precedente Sheets); issue de hardening separada; RLS ENABLE + NOT_FORCED (app-level tenant scoping no CRUD).
3. **Cookie `qk_login` carregando tenant** — assinado (HMAC); o callback confia só no que está assinado, valida a config pelo tenant do cookie. Testar adulteração (tenant trocado no cookie → assinatura falha).
4. **Cache de runtime stale** — invalidar no PUT/DELETE + TTL.
5. **Fallback perigoso:** um `?org` sem config NÃO deve cair no OIDC global (confusão de identidade). Erro explícito.
