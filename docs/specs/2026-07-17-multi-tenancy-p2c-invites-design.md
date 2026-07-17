# P2c — Convites de time (token → membership) (cloud-only)

**Status:** design (aguardando revisão; decisões de produto tomadas no /loop noturno, flagadas pra revisão de manhã)
**Data:** 2026-07-17
**Fase:** P2c de P2 (LUC-23). Fecha, junto do P2d (OIDC por-tenant, LUC-25), o que falta do P2 (LUC-7). Depende de P1/P2a/P2b (mergeados). Base `main @ dca5be8`. Cloud-only.

## Objetivo

Um Owner/Admin de um tenant convida alguém (e-mail + papel); o convite gera um **link com token**; quem abre o link, autentica via OIDC e aceita, ganha `Membership` no tenant com o papel do convite. Sem envio de e-mail (o quark não tem infra de e-mail) — a entrega é compartilhando o link. O OSS não muda.

## Contexto (do mapa de código)

- `Membership { user_id, tenant_id, role, created }` (`src/tenant.rs:46`), `Role {Owner,Admin,Member,Viewer}` (`:37`), `role_scopes` (`:55`) — **Owner e Admin colapsam pra `Scope::Full`**; não há scope "gerir membros" (autorização de gestão é no handler, comentado em `tenant.rs:57`). `memberships`/`tenants`/`users` são tabelas de identidade no **pool pelado** (fora do RLS por-tenant), NÃO em `TENANT_OWNED_TABLES`. `put_membership` é upsert `ON CONFLICT (user_id,tenant_id) DO UPDATE SET role` (`postgres.rs:1853`).
- Precedente: `admin_tenants_create` (`api.rs:1837`) — guard `!multi_tenant→404`, `session_user_id`, rate-limit, `put_*`, `set_session_tenant`. `session_user_id` (`:1787`) resolve o user sem o scope-check do `admin_guard` (um não-membro precisa poder aceitar). `admin_workspace_switch` (`:1892`) valida membership antes de trocar.
- Token: `generate_token()` = `qtok_`+32 base62 via `getrandom` (`auth.rs:67`), `hash_token()` = SHA-256 hex (`:80`); **só o hash é persistido**, plaintext devolvido uma vez (`admin_tokens_create`, `api.rs:3336`). Lookup por hash antes de saber o tenant: `get_api_token_by_hash`/`get_session_by_hash`.
- `admin_guard(st, headers, Scope::Full)` no cloud re-deriva scopes de `role_scopes(membership no tenant atual)` a cada request.
- Rotas: lista flat em `router_with_cors` (`api.rs:3610`); gating `if !st.multi_tenant {404}`; `st.ratelimiter.check`.
- Frontend: `router.tsx` — `/login` é a única rota fora do `RequireAuth`. `RequireAuth` manda pro `WorkspaceGate`/`Onboarding` quando `current_tenant==null` — um convidado de primeira viagem não tem workspace, então a rota de aceite precisa ficar **fora** dessa árvore e fazer o próprio check de auth (`/login?next=/invite/:token`). `/admin/me` expõe memberships+current_tenant; não existe endpoint de "listar membros de um tenant" (novo).

## Decisões travadas (minhas, no /loop; validar de manhã)

1. **Convite ligado ao e-mail.** O título do LUC-23 é "convites por e-mail"; o OIDC entrega e-mail verificado. O convite guarda `email`; o accept exige `User.email == invite.email` (case-insensitive). Impede alguém redimir um convite endereçado a outro.
2. **Single-use + expiry.** `accepted_at IS NULL` e `now <= expires`; expiry default **7 dias**. Convite aceito/expirado → tratado como inexistente (410/404), sem re-conceder membership (anti-replay de link vazado).
3. **Quem convida = Owner ou Admin** (`admin_guard(Scope::Full)`). Member/Viewer não convidam.
4. **Papéis convidáveis: Admin, Member, Viewer — NÃO Owner.** Owner só é setado na criação do tenant; transferência de dono fica fora do P2c.
5. **Já-membro → 409** (`already a member`), sem mudar o papel via replay de convite (mudança de papel é ação explícita, fora do P2c). O accept checa `get_membership` antes.
6. **Split:** P2c-backend (tabela + endpoints + token) e P2c-frontend (página de Membros + rota de aceite). Este spec cobre os dois; frontend vira plano/execução separada (e pode ficar junto do P3-frontend pendente).

## Arquitetura (backend)

### Tabela `invites` (cloud-only; TENANT_OWNED + NOT_FORCED)

```
invites(
  id           BIGINT PRIMARY KEY,        -- nextval quark_invite_id_seq
  tenant_id    BIGINT NOT NULL,           -- tenant do convite
  email        TEXT NOT NULL,             -- e-mail convidado (lowercase)
  role         TEXT NOT NULL,             -- Admin|Member|Viewer (nunca Owner)
  token_hash   TEXT NOT NULL,             -- SHA-256 do token (plaintext nunca guardado)
  invited_by   BIGINT NOT NULL,           -- user_id de quem convidou
  created      BIGINT NOT NULL,
  expires      BIGINT NOT NULL,           -- created + 7d
  accepted_at  BIGINT,                    -- NULL = pendente
  accepted_by  BIGINT                     -- user_id que aceitou
)
```
Índice `invites_token_hash_idx` (lookup por hash). Vai em `TENANT_OWNED_TABLES` (ganha coluna `tenant_id`+RLS ENABLE) **e** em `NOT_FORCED` (o accept faz lookup por token_hash antes de saber o tenant, no pool pelado — como `api_tokens`). Sequência `quark_invite_id_seq` + entrada em `reset_for_tests`.

### Store trait

`create_invite(&Invite)`; `get_invite_by_hash(hash, now) -> Option<Invite>` (bare; filtra `accepted_at IS NULL AND expires >= now`); `mark_invite_accepted(id, user_id, now)`; `list_invites(tenant) -> Vec<Invite>` (tenant-scoped, pra a UI); `delete_invite(tenant, id)` (revogar); `next_invite_id`. Tipo `Invite` em `src/tenant.rs` (ou `src/invite.rs`).

### Endpoints (`/admin/invites`, cloud-only)

- `POST /admin/invites {email, role}` — `admin_guard(Scope::Full)` (Owner/Admin). Valida role ∈ {Admin,Member,Viewer} (rejeita Owner → 400). Gera token, `create_invite(pending, expires=now+7d)`. **Retorna o link de aceite com o token plaintext uma vez** (`{url, token, email, role, expires}`). Rate-limited.
- `GET /admin/invites` — lista os convites pendentes do tenant (sem o token; mostra email/role/expires/status). Owner/Admin.
- `DELETE /admin/invites/:id` — revoga (tenant-scoped). Owner/Admin.
- `POST /admin/invites/:token/accept` — autenticado (`session_user_id`, NÃO `admin_guard` — o convidado pode não ter membership ainda). `get_invite_by_hash(hash_token(token), now)`; ausente/expirado/aceito → 404/410. Checa `User(email) == invite.email` (senão 403). Checa `get_membership(user, invite.tenant)` já existe → 409. Senão `put_membership(user, invite.tenant, invite.role)` + `mark_invite_accepted` + `set_session_tenant(invite.tenant)`. Rate-limited (superfície de adivinhação de token).

### Segurança

Token nunca guardado em claro (hash SHA-256). Accept nunca confia em tenant/role do cliente — só na linha do convite. Single-use (`accepted_at`) + expiry. E-mail binding. Rate-limit no accept. `invites` em NOT_FORCED mas o create/list/delete são tenant-scoped pelo `Principal` (o accept é bare por design). Papel nunca é Owner via convite.

## Arquitetura (frontend, P2c-frontend)

- **Página "Membros"** (nova rota sob o `Shell`, cloud-only): lista convites pendentes (email/role/status) + botão "convidar" (dialog: email + role) que mostra o link a copiar; revogar. (Listar membros atuais fica pra quando houver endpoint; P2c cobre convites.)
- **Rota de aceite** `/invite/:token` **fora** do `RequireAuth` (o convidado não tem workspace ainda): faz o próprio check via `useMe`; não autenticado → `/login?next=/invite/:token`; autenticado → mostra o convite (tenant/role) e botão aceitar → `POST /admin/invites/:token/accept` → em sucesso invalida `me` + entra no workspace. Erros 403/404/409/410 com mensagem clara.
- i18n EN+PT; Vitest.

## Escopo

**Dentro:** tabela `invites`+migração; store methods; `/admin/invites` (create/list/revoke/accept) com token hasheado, single-use, expiry, e-mail binding; página de Membros + rota de aceite; i18n. Cloud-only.

**Fora:** envio de e-mail (só link); transferência de Owner; mudança de papel de membro existente; remover membro de um tenant (só revogar convite pendente); listar membros ativos (precisa endpoint novo — follow-up); billing/seats. OSS não muda. OIDC por-tenant = P2d/LUC-25 (separado).

## Testes

- **Accept feliz:** convidado autentica, e-mail bate → vira membro com o papel certo, convite marcado aceito, sessão no tenant.
- **Segurança:** token errado/expirado/já-aceito → 404/410, sem membership; e-mail não bate → 403; já-membro → 409; convite pra Owner → 400 no create; token só serve uma vez (segundo accept → 404/410); Member/Viewer não conseguem criar convite (403).
- **Tenant-scoping:** um tenant não lista/revoga convite de outro (RLS + Principal; verificar não-superuser).
- **Paridade OSS:** `multi_tenant=false` → todos os `/admin/invites` 404; suíte atual passa igual.
- Postgres gated (`QUARK_TEST_DATABASE_URL`), verificação como não-superuser em cloud; `-j1`; SEM `CREATE INDEX CONCURRENTLY`. Frontend: Vitest.

## Riscos

1. **Replay de link vazado** re-concedendo acesso. Mitigação: single-use (`accepted_at`) + expiry + e-mail binding; teste do segundo-accept.
2. **Confiar em tenant/role do token.** Mitigação: só a linha do convite decide; accept nunca lê tenant/role do cliente; teste.
3. **`put_membership` é upsert** → aceitar convite de quem já é membro mudaria o papel silenciosamente. Mitigação: checar `get_membership` antes e 409 se já-membro.
4. **Convidar Owner** = escalonamento. Mitigação: role convidável nunca é Owner (400 no create).
5. **Convidado preso no `Onboarding`** se a rota de aceite ficar dentro do `RequireAuth`. Mitigação: rota fora da árvore, com auth próprio + `?next=`.
