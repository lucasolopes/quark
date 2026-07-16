# P2b — Signup, criação de tenant e workspace atual

**Status:** design aprovado (aguardando revisão do spec antes do plano)
**Data:** 2026-07-16
**Sub-fase:** P2b de P2 (P2a enforcement mergeado; P2c convites; P2d OIDC por tenant). Depende de P1a+P1b+P2a (mergeados). Base `main @ 942176f`.

## Objetivo

Fazer o multi-tenant **usável**: no modo cloud, qualquer identidade autenticada (via OIDC) pode se cadastrar e **criar o próprio workspace** (vira Owner), o tenant atual vive na sessão, e o usuário troca de workspace na UI. A autorização no cloud passa a vir do **papel do usuário no tenant atual** (não mais do grupo global do IdP). O OSS (single-tenant) não muda.

## Contexto (estado atual, pós-P2a)

- `admin_guard` retorna `Principal { tenant, user_id, scopes }`; no cloud o FORCE RLS + tenant-tx já isolam por `app.tenant_id` (P2a). Hoje as `scopes` vêm do mapa **grupo-do-IdP → scope** (`oidc.rs map_scopes`), global.
- Login OIDC (P1b) faz upsert de `User` (por `subject`) + `Membership(user, DEFAULT_TENANT, role)` e cria `Session { tenant_id, user_id, scopes }`. Ou seja, **todo login hoje entra no tenant 0** — que é o certo pro OSS, errado pro cloud self-serve.
- `Membership` é M:N (`memberships(user_id, tenant_id, role)`), `role ∈ {Owner,Admin,Member,Viewer}`, `role_scopes()` já existe.
- Flag `QUARK_MULTI_TENANT` (P2a) distingue OSS/cloud.

## Decisões travadas

1. **Signup aberto self-serve** (com o usuário): qualquer login OIDC autenticado pode criar seu workspace; 1º login sem membership → onboarding "criar workspace" (vira Owner); um usuário pode ter/criar vários workspaces (M:N), com seletor.
2. **Autorização no cloud = papel no tenant atual.** `admin_guard` no cloud resolve o papel do usuário na `session.tenant_id` (via membership) → `role_scopes(role)`. **No OSS segue o mapa grupo→scope de hoje** (tenant 0). É aqui que o papel dirige a autorização (o P1b só gravava).

## Arquitetura

### Fluxo de login por modo

- **OSS (flag off):** inalterado. Login → membership no tenant 0, sessão no tenant 0, scopes do grupo do IdP (ou o admin token). Comportamento de hoje.
- **Cloud (flag on):** login faz upsert do `User` por `subject` (como hoje) mas **NÃO** cria membership automática no tenant 0. A sessão é criada sem tenant "efetivo" ainda; o `/admin/me` reporta as memberships do usuário:
  - 0 memberships → o painel manda pro onboarding "criar workspace".
  - 1 membership → vira o tenant atual da sessão.
  - N memberships → o painel mostra o seletor (usuário escolhe o atual).

### Sessão carrega o workspace atual

`session.tenant_id` = o workspace atual. Um endpoint **`POST /admin/workspace/switch`** (nome a confirmar no plano) recebe um `tenant_id`, **valida que o usuário tem membership nele**, e atualiza a sessão. `admin_guard` no cloud usa `session.tenant_id` + a membership pra montar o `Principal` (tenant + papel → scopes).

### Criar tenant (self-serve)

**`POST /admin/tenants`** (cloud, autenticado): cria `Tenant { name, slug }` + `Membership(user, novo_tenant, Owner)`, e passa a ser o tenant atual da sessão. `slug` único (já é no schema). Rate-limit/anti-abuso no create (reusa o rate limiter). No OSS esse endpoint fica desabilitado (só existe o tenant 0).

### `admin_guard` no cloud

O guard passa a: (env admin token → platform admin, como hoje) OU (API token → seu tenant, como hoje) OU (sessão OIDC → resolve a membership do usuário no `session.tenant_id`; `scopes = role_scopes(membership.role)`; se não há membership no tenant atual → 403). Preserva o contrato de status. **No OSS o caminho é o de hoje** (grupo→scope, tenant 0).

### Frontend

- Tela de **onboarding "criar workspace"** (quando `me()` retorna 0 memberships no cloud).
- **Seletor de workspace** no Shell (lista as memberships, troca via o endpoint de switch, re-carrega o estado).
- `me()` passa a retornar as memberships + o tenant atual.

## Escopo

**Dentro (P2b):** login cloud sem auto-membership; `POST /admin/tenants` (criar workspace); `session.tenant_id` como workspace atual + endpoint de switch (valida membership); `admin_guard` cloud por papel; `/admin/me` com memberships + atual; UI de criar-workspace + seletor. **Provável split:** P2b-backend (tudo acima menos UI) e P2b-frontend (as 2 telas).

**Fora:** convites (P2c); OIDC por-tenant/config por tenant (P2d); billing (LUC-41); Host→tenant (P3). O OSS não muda.

## Testes

- **Auth por papel no cloud (chave):** um usuário com membership `Viewer` no tenant atual só tem scopes de leitura; `Member` escreve; `Owner`/`Admin` full; sem membership no tenant atual → 403. Contrato de status do `admin_guard` preservado.
- **Signup:** `POST /admin/tenants` cria Tenant + Membership Owner + vira o atual; slug duplicado rejeitado; endpoint desabilitado no OSS.
- **Switch:** trocar pra um tenant onde o usuário TEM membership funciona; pra um onde NÃO tem → negado (não troca).
- **Login cloud sem auto-tenant-0:** um login novo no cloud não ganha membership no tenant 0; `me()` reporta 0 memberships → sinaliza onboarding.
- **Paridade OSS:** flag off = login/tenant-0/grupo→scope idênticos; suíte atual passa igual.
- **Isolamento com papel:** combinado com o FORCE RLS do P2a — dois workspaces do mesmo usuário não vazam entre si.
- Frontend: Vitest pras 2 telas; gate `-j1`; PG gated. Verificação: arm gated como não-superuser + fluxo signup→switch ponta-a-ponta.

## Riscos

1. **Mudança da fonte de autorização no cloud** (grupo→scope vira papel-da-membership) — security-critical; preservar o contrato de status e o comportamento OSS byte-a-byte; teste cobrindo cada papel + o caso "sem membership no tenant atual → 403".
2. **Switch de workspace sem validar membership** = escalada entre tenants. Mitigação: o switch SEMPRE valida membership antes de mudar a sessão; teste negativo.
3. **Login cloud não pode cair no tenant 0** por engano (senão um usuário novo veria/mexeria em dado do tenant default). Mitigação: no cloud, zero membership automática; teste.
4. **`admin_guard` é o mesmo código pros dois modes** — um `if multi_tenant` mal colocado quebra o OSS. Mitigação: paridade OSS testada.
