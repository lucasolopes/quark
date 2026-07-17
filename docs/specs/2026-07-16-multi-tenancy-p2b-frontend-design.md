# P2b-frontend — Onboarding (criar workspace) + seletor de workspace

**Status:** design (dentro do P2b já aprovado; UI dos endpoints do P2b-backend)
**Data:** 2026-07-16
**Fase:** P2b-frontend (segue o P2b-backend, mergeado `889e6b4`). Branch `feat/multi-tenant-p2b-frontend` (off main `889e6b4`).

## Objetivo

As duas telas que faltam pra o multi-tenant cloud ser usável de ponta a ponta pelo painel: (1) **onboarding "criar workspace"** quando um usuário cloud autenticado ainda não tem workspace, e (2) o **seletor de workspace** no cabeçalho pra alternar entre os workspaces de que ele é membro. Consome os endpoints do P2b-backend. O OSS (single-tenant) não muda — nada disso aparece.

## Contexto (frontend atual + backend pronto)

- `RequireAuth` (`web/src/app/RequireAuth.tsx`) usa `api.me()`; `authenticated` → renderiza o app, senão → `/login`.
- `Shell` (`web/src/app/Shell.tsx`) tem a sidebar + um header (canto direito: idioma/tema/logout).
- `MeResponse` (`web/src/lib/types.ts:68`) hoje: `{ authenticated, oidc_enabled, display?, scopes? }`.
- Backend (P2b-backend, mergeado): `/admin/me` agora retorna `memberships: [{tenant_id,name,slug,role}]` + `current_tenant: number|null` (cloud); `POST /admin/tenants {name,slug}`; `POST /admin/workspace/switch {tenant_id}`. Tudo 404 no OSS.
- Padrão do painel: React 19 + Tailwind + componentes em `web/src/components/ui/*` (Button/Dialog/Input/etc.), i18n `web/src/i18n` (EN + PT-BR), estado via `@tanstack/react-query` (`web/src/lib/queries.ts`), API em `web/src/lib/api.ts`. **Casar com esse estilo** — não inventar design novo.

## Arquitetura (UI)

### Tipos + API (`lib/types.ts`, `lib/api.ts`)

- `MeResponse` ganha `memberships?: Membership[]` e `current_tenant?: number | null`, com `Membership = { tenant_id: number; name: string; slug: string; role: string }`. Opcionais → OSS (que não manda) continua compatível.
- `api.ts`: `createWorkspace(name, slug): Promise<{id:number;name:string;slug:string}>` (`POST /admin/tenants`), `switchWorkspace(tenant_id): Promise<void>` (`POST /admin/workspace/switch`). Ambos mandam o header `x-quark-csrf` + `credentials:include` como as outras mutações.

### Gate de onboarding (`RequireAuth`)

Quando **cloud** (o `me()` traz o campo `memberships`) E autenticado E **sem workspace atual** (`current_tenant == null` / `memberships` vazio) → renderiza a tela de **onboarding "criar workspace"** em vez do app. Autenticado COM workspace → app normal. OSS (sem `memberships` no payload) → comportamento de hoje, o gate é no-op. (Login por token break-glass segue entrando direto, como hoje.)

### Tela "criar workspace" (`web/src/routes/CreateWorkspace.tsx`, nova)

Form simples no padrão dos dialogs existentes: campo **nome** + **slug** (slug auto-derivado do nome, editável; validação client-side de formato). Botão "criar". Chama `createWorkspace`; em sucesso → invalida o query `["me"]` (o backend já trocou o tenant atual da sessão pro novo) → o `RequireAuth` re-resolve e cai no app. Erro 409 (slug em uso) → mensagem inline "esse slug já existe". Erro 429 → "muitas tentativas, tente em instantes".

### Seletor de workspace (no header do `Shell`)

Um dropdown (usando o `dropdown-menu` de `components/ui`) no header, à esquerda do idioma/tema, mostrando o **workspace atual** (nome) e a lista das `memberships` (nome + papel); selecionar um chama `switchWorkspace(tenant_id)` → em sucesso invalida **todas** as queries de dados (links/stats/tokens/etc. são por-tenant) + o `["me"]` e re-carrega. Item final "**+ Criar workspace**" → leva pra tela de criar. **Só aparece no cloud com `memberships`**; no OSS (sem o campo) não renderiza.

### i18n + testes

- Chaves EN + PT-BR pras duas telas (título, labels, erros, "criar workspace", "trocar workspace").
- Vitest: o gate (0 memberships → onboarding; ≥1 → app; OSS → app sem seletor); o form de criar (sucesso invalida me; 409 mostra erro); o seletor (lista memberships, troca invalida queries).

## Escopo

**Dentro:** tipos + métodos de API; gate de onboarding no `RequireAuth`; tela criar-workspace; seletor de workspace no Shell; i18n EN+PT; Vitest.

**Fora:** convites (P2c) — a tela de convite/aceite; OIDC por-tenant (P2d); billing. Nenhuma mudança de backend (já pronto). OSS não muda.

## Testes / verificação

- Vitest cobre o gate, o form e o seletor (mockando `api.me/createWorkspace/switchWorkspace`).
- `npm run build` + `npm run test` (Vitest) + `npm run lint` verdes.
- Paridade OSS: sem `memberships` no `me()`, o painel renderiza igual a hoje (sem onboarding, sem seletor) — teste explícito.
- Verificação ponta-a-ponta (manual/controller quando possível): buildar a SPA e checar o fluxo signup→app→switch contra o backend cloud.

## Riscos

1. **Gate de onboarding não pode prender o OSS nem o login por token.** Mitigação: o gate só dispara quando `memberships` vem no payload (cloud) e está vazio; token break-glass entra direto (como hoje); teste de paridade OSS.
2. **Trocar workspace sem invalidar o cache** mostraria dados do tenant anterior. Mitigação: o switch invalida TODAS as queries de dados + `me`, não só a sessão.
3. **Compat do `MeResponse`:** campos opcionais; consumidores atuais (que só leem `authenticated`) não quebram.
