# P2d-frontend — Login ciente de organização + convite modelo-B (cloud) (LUC-53)

**Status:** design (aguardando aprovação antes de implementar). Base `main @ 10d7db2` (P2e mergeado). Só frontend (`web/`), cloud-aware, sem backend novo.

## Objetivo

Fechar a ponta visual do login OIDC por-tenant. O grosso do que o usuário quer — logar uma vez, descobrir os workspaces e pular entre eles sem relogar — **já existe** (P2b/P2c-frontend: `RequireAuth` → `WorkspaceGate` com 0=onboarding / 1=auto-switch / N=seletor, `WorkspaceSwitcher`, criar/trocar workspace, aceitar convite). Este spec cobre só o que falta pro modelo B (Keycloak/SSO por-empresa do P2e) funcionar de ponta a ponta no frontend.

## Contexto (verificado no código)

- **Backend do login** (`src/api.rs::oidc_login`): `/admin/login` sem `org` → OIDC **global** (login compartilhado); `/admin/login?org=<slug>` → OIDC **daquele tenant** (404 genérico se slug desconhecido ou tenant sem config — anti-enumeração). O callback e a sessão são os do P2d-A, intocados.
- **Convite modelo B** (`src/api.rs::admin_invites_accept`, P2e): com Keycloak ligado, o accept **não cria membership** — responde `200 {"status":"login_required","login_url":"/admin/login?org=<slug>"}`. A membership nasce no 1º login pelo claim de grupo. (Sem Keycloak = modelo A, accept cria membership como hoje.)
- **Frontend hoje:**
  - `web/src/lib/api.ts`: `oidcLoginUrl()` → `` `${BASE}/admin/login` `` (sem org). `me()`, `createWorkspace`, `switchWorkspace` já existem. `acceptInvite` (via `useAcceptInvite`) hoje descarta o corpo da resposta.
  - `web/src/routes/Login.tsx`: campo de token + botão "entrar com provedor" (chama `oidcLoginUrl()`, sem org) quando `me().oidc_enabled`.
  - `web/src/routes/AcceptInvite.tsx`: em qualquer sucesso do accept navega pra `/links`. **Não** conhece `login_required` → no modelo B o usuário cairia no `WorkspaceGate` com zero workspaces.
  - `MeResponse` (`web/src/lib/types.ts`) já traz `memberships`/`current_tenant` (cloud) e `oidc_enabled`.

## O gap (só isto)

1. **AcceptInvite tratar o modelo B:** detectar a resposta `login_required` e **redirecionar o browser** pro `login_url` absoluto (o `/admin/login?org=<slug>` na origem da API), em vez de tratar como sucesso e ir pra `/links`.
2. **Login ciente de `?org=`:** `Login.tsx` lê `org` da própria query string (bookmark ou redirect). Com `org` presente → mostra "Entrar em `<slug>`" e o botão SSO vai pro `oidcLoginUrl(org)`; sem `org` → login compartilhado de sempre. Dá ao usuário de SSO-por-empresa **que volta** um caminho de entrada.

**Fora de escopo (deferido):** derivar o `org` do hostname real (`acme.quark.app` → `org=acme`) e servir o painel no host do tenant — vira o tijolo seguinte (precisa de infra de deploy). Também fora: mudar qualquer coisa de backend; telas de gestão de domínio (já resolvido automático).

## Arquitetura

### 1. `api.ts`
- `oidcLoginUrl(org?: string)`: sem `org` → `` `${BASE}/admin/login` `` (inalterado); com `org` → `` `${BASE}/admin/login?org=${encodeURIComponent(org)}` ``.
- `acceptInvite` passa a **retornar o corpo parseado** da resposta 200 (`{ status?: string; login_url?: string }`) em vez de `void`, pra o caller distinguir `login_required` de um accept-cria-membership (modelo A, sem `status`). Um accept modelo-A continua devolvendo o corpo atual (sem `status`) → tratado como sucesso.

### 2. `AcceptInvite.tsx`
- No `onSuccess` do `acceptInvite`: se `data?.status === "login_required" && data.login_url` → `window.location.assign(<BASE + login_url>)` (navegação de página inteira pro endpoint de redirect da API, que leva ao IdP do tenant). O `login_url` vem relativo (`/admin/login?org=...`); prefixar com a base da API (mesma base do `req`/`oidcLoginUrl`). Caso contrário (modelo A, sem `status`) → `navigate("/links")` como hoje.
- Demais estados (403 mismatch, 409 já-membro, 404/410 expirado, 429) inalterados.

### 3. `Login.tsx`
- Ler `org` de `useSearchParams()` (`?org=<slug>`). Guardar em uma variável.
- Se `org` presente: o botão do provedor vira "Entrar em `<slug>`" (copy i18n com o slug) e chama `oidcLoginUrl(org)`. Mostrar o `org` no cabeçalho pra o usuário saber onde está entrando.
- Se `org` ausente: comportamento atual (botão compartilhado + campo de token), sem regressão.
- O campo de token (break-glass) permanece nos dois casos.

### 4. i18n
- Chaves novas em `web/src/i18n/pt-BR.ts` e `en.ts`: `login.orgButton` (ex.: "Entrar em {org}"), `login.orgHeader` (ex.: "Organização: {org}"), e (se preciso) `accept.redirectingToLogin`. Interpolação de `{org}` no padrão já usado no projeto (checar o helper de i18n antes de assumir o formato).

## Testes (Vitest, `web/`)

- **api:** `oidcLoginUrl("acme")` → contém `/admin/login?org=acme`; `oidcLoginUrl()` → `/admin/login` sem query. `acceptInvite` retorna o corpo parseado (com e sem `status`).
- **AcceptInvite:** resposta `{status:"login_required",login_url:"/admin/login?org=acme"}` → dispara navegação de página pro `BASE + login_url` (mockar `window.location.assign`), **não** vai pra `/links`; resposta modelo-A (sem `status`) → navega pra `/links` (regressão). Estados de erro inalterados.
- **Login:** com `?org=acme` na URL → renderiza "Entrar em acme" e o clique chama `oidcLoginUrl("acme")`; sem `org` → botão compartilhado chama `oidcLoginUrl()` sem org (regressão). Campo de token presente nos dois.
- Suíte web existente continua verde (sem regressão em `Login.test.tsx`/`AcceptInvite.test.tsx`).

## Riscos

1. **Cross-origin do redirect:** o painel e a API são origens distintas (`app.quarkus.com.br` × API). O `login_url` do convite é relativo à API; prefixar com a base da API (a mesma que `oidcLoginUrl`/`req` usam). Uma navegação de página inteira (`window.location.assign`) evita CORS (é redirect do browser, não fetch).
2. **`org` da query é dado não-confiável:** é só um hint de UX; o backend valida o slug e devolve 404 genérico pra slug inexistente/sem-config (anti-enumeração já existente). O frontend não deve vazar se o slug existe — apenas encaminha; a decisão é do backend.
3. **Usuário modelo-B que volta sem bookmark com `?org=`:** só terá o caminho compartilhado (que é o realm errado pra ele). Mitigação real = o tijolo deferido (host deriva o org). Documentar como limitação conhecida até lá.
4. **Regressão no modelo A:** um accept modelo-A não traz `status`; o caller deve tratar ausência de `status` exatamente como o sucesso de hoje. Teste de regressão garante.
