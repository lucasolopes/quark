# P1b — Auth binding a tenant (plumbing) + carry-overs do P1a

**Status:** design (aguardando revisão do spec antes do plano)
**Data:** 2026-07-16
**Sub-projeto:** P1b de 4. Depende do P1a (mergeado, `dd42200`). Antecede P2 (onboarding cloud), P3 (domínios), P4 (integrações/analytics por tenant).

## Objetivo

Fazer a **autenticação saber a qual tenant e usuário cada request pertence**, e fechar os carry-overs de PK do P1a. É **plumbing puro**: tudo ainda resolve pro tenant 0, o comportamento observável do OSS não muda, e ninguém cria tenants aqui (isso é P2). Depois do P1b, o sistema tem `Principal { tenant, user, scopes }` em todo caminho admin, as entidades `User`/`Membership` ficam vivas (populadas no login), e as tabelas que faltavam ganham PK tenant-correto.

## Decisão de fronteira (com o usuário)

**Plumbing puro.** No modo cloud, antes do P2, toda credencial resolve pro tenant 0. O P1b monta o encanamento (credenciais carregam tenant/usuário, o guard resolve o `Principal`); **criar tenants e associar usuários é 100% do P2**.

## Decisão de arquitetura: `FORCE RLS` vai pro P2

As policies RLS já estão **definidas** no P1a. Ligar o `FORCE ROW LEVEL SECURITY` + dirigir `app.tenant_id` via `begin_tenant_tx` **não** entra no P1b, e sim no P2, porque: (a) com um tenant só, o RLS não tem nada a isolar — é inerte; (b) forçá-lo exige rodar **toda** query dentro de uma transação com `SET LOCAL app.tenant_id`, custo real por request, sem benefício observável no P1b; (c) a camada enforçada de isolamento continua sendo o `WHERE tenant_id`/prefixo app-level, já verificado. O P2 liga o `FORCE` junto com o código que cria o 2º tenant, que é quando ele passa a proteger algo de fato.

## Arquitetura

### `Principal` e o `admin_guard`

Hoje `admin_guard(st, headers, required) -> Result<(), StatusCode>` só autoriza. Passa a devolver quem é:

```rust
pub struct Principal {
    pub tenant: TenantId,       // no P1b: sempre DEFAULT_TENANT
    pub user_id: Option<u64>,   // Some(id) via sessão OIDC; None via env admin token
    pub scopes: Vec<Scope>,     // as scopes efetivas da credencial
}
// admin_guard(st, headers, required) -> Result<Principal, StatusCode>
```

- **Contrato de status inalterado:** a lógica de 401/403/404/429/503 (`src/api.rs:1273-1364`) é preservada exatamente; só o retorno do caminho de sucesso muda de `()` pra `Principal`.
- **env admin token** → `Principal { tenant: DEFAULT_TENANT, user_id: None, scopes: [Full] }` (admin de plataforma / tenant 0).
- **API token** → `Principal { tenant: token.tenant_id, user_id: None, scopes: token.scopes }` (no P1b `token.tenant_id` é sempre 0).
- **sessão OIDC** → `Principal { tenant: session.tenant_id, user_id: Some(session.user_id), scopes: session.scopes }`.

### Handlers adotam o `ScopedStore`

Todo handler admin passa a: `let p = admin_guard(...).await?;` e então `let store = st.store.clone().for_tenant(p.tenant);` — usando o handle escopado em vez do `DEFAULT_TENANT` cru que o P1a deixou nos call sites. Isso realiza a ergonomia de segurança que o P1a projetou (impossível esquecer o tenant) e conecta o tenant resolvido ao acesso a dados. No P1b `p.tenant` é sempre 0, mas o encanamento fica pronto pro P2 injetar o tenant real.

### `ApiToken` e `Session` carregam tenant + usuário

Os structs (`src/auth.rs`) ganham:
- `ApiToken.tenant_id: TenantId` (as colunas já existem no schema desde o P1a; agora o struct as lê/escreve).
- `Session.tenant_id: TenantId` + `Session.user_id: u64`.

No P1b são sempre `DEFAULT_TENANT` / o usuário do login. Serialização com `#[serde(default)]` (LMDB) + default 0 (Postgres) pra registros pré-P1b lerem como tenant 0.

### `User`/`Membership` ficam vivos no login OIDC

No callback OIDC (`src/oidc.rs`), após validar o token e mapear scopes (`map_scopes`), o P1b:
1. **upsert `User`** por `subject` (cria se novo; `next_user_id`).
2. **upsert `Membership`** `(user, DEFAULT_TENANT, role)` — `role` alinhado ao mesmo grupo do IdP que hoje deriva as scopes: `admin_value` → `Admin`, `readonly_value` → `Viewer`.
3. cria a `Session` com `tenant_id = DEFAULT_TENANT` e `user_id` do usuário upsertado.

**Autorização NÃO muda de fonte:** o `Principal.scopes` continua vindo das scopes da credencial (`session.scopes`/`token.scopes`/`[Full]` do env) — exatamente como hoje —, NÃO recomputado a partir do `role`. O `role` na membership é um registro paralelo, alinhado às scopes pra não divergir, que só passa a **dirigir** autorização no P2. `Owner` não é atribuído no P1b (é o criador do tenant → P2, no signup).

**Adiciona `Role::Viewer`** ao enum do P1a (`src/tenant.rs`): `role_scopes(Viewer) = [LinksRead, Analytics]`, que casa exatamente com o que o grupo readonly do OIDC concede hoje. Esse é o gatilho que o design do P1a previu ("adicionar Viewer só quando o caso read-only aparecer" — apareceu, no grupo readonly). Enum vira `Owner | Admin | Member | Viewer`.

Isso exercita as tabelas `users`/`memberships` (criadas vazias no P1a) pela primeira vez, ainda em single-tenant.

### Carry-overs de PK do P1a (migração testada)

- `sheets_connection`: **dropar o `singleton BOOLEAN PRIMARY KEY`**, PK vira `(tenant_id)`. Migração idempotente: `ALTER TABLE sheets_connection DROP CONSTRAINT IF EXISTS sheets_connection_pkey`, `DROP COLUMN IF EXISTS singleton`, `ADD PRIMARY KEY (tenant_id)` (guardado por checagem de existência). O upsert deixa de usar `singleton`.
- `wellknown_documents`: PK vira `(tenant_id, name)` (`DROP CONSTRAINT ... _pkey` + `ADD PRIMARY KEY (tenant_id, name)`); `put_wellknown` usa `ON CONFLICT (tenant_id, name)`.
- LMDB já chaveia os dois por tenant — sem mudança lá.
- **Migração validada** como no P1a: dry-run sobre dump do schema+dados reais de prod, confirmando ALTER limpo e dados preservados. Os `CREATE INDEX` continuam **sem** `CONCURRENTLY` (testado e reprovado no P1a — deadlock sob o advisory lock do boot).

## Escopo

**Dentro do P1b:** `Principal` + `admin_guard` retornando-o; `ApiToken`/`Session` carregando tenant+user; `User`/`Membership` populados no login OIDC; adoção do `ScopedStore` nos call sites admin; PK reworks de `sheets_connection` e `wellknown_documents` com migração testada.

**Fora do P1b:** flag `QUARK_MULTI_TENANT` e qualquer branch de comportamento cloud (P2 — nada no P1b se ramifica por modo ainda); `FORCE RLS` + `begin_tenant_tx` (P2); signup, convites, seletor de workspace, OIDC por-tenant (P2); relay de webhooks por-tenant (P2); `Host→tenant`/domínios (P3); Sheets/ClickHouse por tenant (P4).

## Testes

- **Paridade OSS (chave):** toda a suíte atual (lib + api_it + tenant_isolation) passa idêntica — resolver pro tenant 0 não muda nada observável. O contrato de status do `admin_guard` (401/403/404/429/503) tem teste que continua verde.
- **Principal:** env admin token → `tenant 0, user None, Full`; API token → tenant/scopes do token; sessão OIDC → tenant 0 + `user_id` do usuário logado.
- **User/Membership no login:** um callback OIDC simulado cria/upserta o `User` por subject e a `Membership (user, 0, role)`; um 2º login do mesmo subject não duplica.
- **PK reworks:** round-trip de `sheets_connection`/`wellknown` sob o novo PK; migração idempotente (rodar `init_schema` 2× sem erro) + dry-run sobre dump de prod (Postgres gated por `QUARK_TEST_DATABASE_URL`).
- Gate `-j1`/`CARGO_BUILD_JOBS=1`.

## Riscos

1. **Refactor do `admin_guard` + call sites** — mudar o retorno pra `Principal` toca todo handler admin; o risco é regressão no contrato de status. Mitigação: preservar a lógica de erro linha-a-linha, teste de contrato de status verde.
2. **Migração de PK em tabela existente** — `DROP CONSTRAINT`/`ADD PRIMARY KEY` em `sheets_connection`/`wellknown` de prod. Mitigação: idempotência + dry-run sobre dump real de prod (o mecanismo já usado e validado no P1a).
3. **Registros pré-P1b** — tokens/sessões serializados antes do P1b não têm `tenant_id`/`user_id`. Mitigação: `#[serde(default)]` (→ tenant 0 / user 0) no LMDB e default de coluna no Postgres; sessões antigas continuam válidas resolvendo pro tenant 0.
