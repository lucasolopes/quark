# P1 — Fundação de multi-tenancy (dados + isolamento)

**Status:** design aprovado (aguardando revisão do spec antes do plano de implementação)
**Data:** 2026-07-16
**Sub-projeto:** P1 de 4 (P2 auth/onboarding cloud · P3 domínios próprios · P4 integrações/analytics por tenant)

## Objetivo

Tornar o quark inteiro consciente de tenant, mantendo **um único binário** que serve dois modos: OSS auto-hospedado (um tenant fixo, N usuários) e cloud (N tenants, cada um com N usuários). O P1 entrega a fundação de dados, isolamento e auth mínima. O OSS deve se comportar **idêntico a hoje**; a cloud ganha o alicerce sobre o qual o P2 liga signup, convites e OIDC multi-tenant.

## Contexto: o estado atual (achados da investigação)

Auditoria de `src/` (2026-07-16) confirmou:

- **Não existe entidade de tenant, usuário ou conta.** Todo dado é um namespace global plano, chaveado por id, alias ou nome.
- **A auth é centrada em credencial, não em identidade:** um token/sessão concede escopos (`src/auth.rs`, `Scope` enum). O `subject` do OIDC existe só como string dentro da `Session`, não como registro persistido.
- **O namespace do short-code é uma permutação Feistel global única** (`src/permute.rs`, 4 rounds sobre 40 bits, chave única `QUARK_KEY`; `src/codec.rs` base62 → 7 chars). A alocação de id é um `SEQUENCE` global no Postgres / contador `meta["next_id"]` no LMDB.
- **`sheets_connection` é explicitamente singleton** (`singleton BOOLEAN PRIMARY KEY`) — "OSS é single-tenant" no comentário. Vira uma-por-tenant.
- **`QUARK_PUBLIC_HOST` é um host único do processo**; wellknown/AASA são chaveados só por nome. (Tratado no P3.)
- Backends: `src/store/mod.rs` (trait, ~40 métodos), `src/store/postgres.rs` (DDL em `init_schema`), `src/store/lmdb.rs` (13 sub-databases).

## Decisões travadas (com o usuário)

1. **Membership muitos-pra-muitos.** Um usuário (identidade global) pode ser membro de N tenants (modelo GitHub/Vercel/Dub), com seletor de workspace. No OSS a tabela simplesmente nunca aponta pro 2º tenant.
2. **Domínios híbridos.** Todos usam um domínio padrão compartilhado por default; cada tenant *pode* plugar o próprio (detalhe no P3). Consequência de namespace: ver abaixo.
3. **Namespace de código global (Feistel intocado).** Os códigos auto-gerados continuam num espaço global único; o `tenant_id` é uma coluna de **posse**, não uma partição do espaço de código. Isso preserva as propriedades calibradas do `permute`/`codec` e elimina vazamento cross-tenant no host compartilhado (id global → código global único). Slugs vaidosos por domínio e resolução `Host→tenant` são do P3.
4. **Isolamento app-level nos dois backends + RLS no Postgres (cloud) como trava fail-closed.**
5. **Um flag de modo**, sem fork de binário; OSS fixa `tenant_id = 0`.

## Arquitetura

### Modelo de dados

Entidades novas:

- **`Tenant`** — `{ id, name, slug, created }`. No OSS existe exatamente um, id `0` ("default"), semeado no boot.
- **`User`** — `{ id, subject, email, display, created }`. `subject` = `sub` do OIDC (imutável), chave natural de identidade. Único por `subject`.
- **`Membership`** — junção muitos-pra-muitos `{ user_id, tenant_id, role, created }`. `role` ∈ `{ Owner, Admin, Member }` **na linha da membership**. PK `(user_id, tenant_id)`.

Papéis → permissões via função `role_scopes(role) -> &[Scope]` em código (reusa o `Scope` enum existente como primitiva de permissão), pra dividir/estender papéis depois sem migração de schema:

- **Owner:** tudo + deletar/transferir o tenant. (≥1 por tenant.)
- **Admin:** gerencia membros, tokens, configs; equivale a `Full` menos gestão do tenant em si.
- **Member:** cria/edita links, vê analytics (`LinksWrite`, `LinksRead`, `Analytics`).

Todo dado tenant-owned ganha `tenant_id`: links, aliases, tokens, sessões, pixels, webhooks (+ deliveries/outbox), analytics (counters/events/meta), health, wellknown, sheets_connection.

### O `Store` trait: handle escopado

Em vez de espalhar um parâmetro `tenant_id` por ~40 assinaturas (fácil esquecer num call site = vazamento), introduzimos um **handle escopado**:

```rust
// no trait Store (ou como extensão):
fn for_tenant(self: Arc<Self>, tenant: TenantId) -> ScopedStore;

// ScopedStore captura o tenant; seus métodos NÃO recebem tenant_id:
impl ScopedStore {
    async fn get_link(&self, id: u64) -> ...;      // tenant implícito
    async fn list_links(&self, ...) -> ...;         // filtrado no tenant capturado
    // ... espelha os métodos tenant-owned do Store
}
```

- Os handlers em `src/api.rs` recebem o handle **já escopado** depois que a auth resolve `(usuário, tenant, papel)`. Torna **impossível** esquecer o filtro num call site.
- O trait interno ainda recebe o `tenant_id` (o backend precisa dele pra montar a query/chave); o handle é a camada ergonômica e de segurança por cima.
- Métodos **globais/infra** ficam fora do handle escopado e no `Store` direto: `gc_sessions`, `try_acquire_health_lease`, `try_acquire_sheets_lease`, `claim_due_deliveries`/`mark_*` (o relay do outbox é poll cluster-wide; o tenant viaja **dentro** da linha, não como filtro de claim), e a resolução de sessão/token por hash (recebem o hash global e **retornam** o tenant).

### Isolamento por backend

**Postgres** (`init_schema`):
- Coluna `tenant_id BIGINT NOT NULL` em cada tabela tenant-owned. Índices/PKs ajustados: onde a unicidade era global e continua global (código no domínio compartilhado, `token_hash`, `delivery_key`, `subject`) mantém; onde é por-tenant (agregações, listagens) o índice ganha `tenant_id` na frente.
- `sheets_connection`: **descontinua o singleton** — PK vira `(tenant_id)`.
- Tabelas novas: `tenants`, `users` (único por `subject`), `memberships` (PK `(user_id, tenant_id)`).
- **RLS**: policies `USING (tenant_id = current_setting('app.tenant_id')::bigint)` nas tabelas tenant-owned, aplicadas com **`SET LOCAL app.tenant_id = $1` por transação** (nunca `SET` puro — conexão pooled herdaria o tenant anterior). Ligado só no modo cloud; no OSS (tenant 0 único) é inócuo.

**LMDB** (`src/store/lmdb.rs`):
- **Prefixo de chave**: `tenant_id` big-endian (u64) + chave atual. Mantém `MAX_DBS = 13` (não é db-por-tenant — inviável com tenant count ilimitado).
- Todos os range-scans (`list_links`, iteradores de tags/folders/aliases) passam a ser **limitados ao prefixo do tenant** em vez de `Bound::Unbounded`.
- Lookups por hash (sessão, token) escaneiam global e o **valor carrega** o tenant.

### Auth (mínimo pro P1)

- `ApiToken` e `Session` ganham `tenant_id` + `user_id`. `token_hash`/`session token_hash` continuam globalmente únicos (aleatórios); a linha carrega o tenant, então `get_api_token_by_hash`/`get_session_by_hash` resolvem o tenant.
- `admin_guard` (`src/api.rs`) mantém **o mesmo contrato de status** (env admin token → API token → sessão OIDC; 401/403/404/429 idênticos) mas agora produz `(usuário, tenant, papel)` em vez de só "autorizado pro escopo X". A checagem de autorização passa a ser `(usuário, tenant)`, não só usuário.
- `QUARK_ADMIN_TOKEN` = admin do tenant 0 (OSS) / plataforma. `map_scopes` do OIDC segue global no P1 (multi-tenant real é P2).
- `require_admin_for_create` mantém o modo "encurtador aberto" (create público quando nem admin token nem OIDC configurados) — resolvendo pro tenant 0.

### Modo OSS vs cloud

- Um flag: `QUARK_MULTI_TENANT` (ausente/`0` = OSS; `1` = cloud), no estilo dos env vars existentes em `src/main.rs`.
- **OSS:** boot semeia o tenant `0`; toda chamada escopada usa essa constante; sem endpoints de provisionamento de tenant; `Host→tenant` é no-op. Comportamento = hoje.
- **Cloud:** o tenant é resolvido por request (pela credencial autenticada no P1; por Host no P3). Endpoints de signup/convite ativam no P2.

### Migração dos dados existentes

- Postgres: migração idempotente que adiciona as colunas `tenant_id` com default `0`, cria `tenants(0,'default')`, e ajusta índices/policies. Dados atuais viram do tenant 0.
- LMDB: os registros existentes (sem prefixo) são lidos como tenant 0. Estratégia: escrita de migração no boot que re-chaveia com prefixo `0`, ou um shim de leitura que trata chave sem prefixo como tenant 0. **Decisão pro plano:** re-chavear no boot (uma vez, sob lease) é mais limpo que shim permanente.

## Escopo

**Dentro do P1:** entidades Tenant/User/Membership + papéis; `tenant_id` em todo dado nos 2 backends; handle escopado no `Store`; RLS (definido, ligado na cloud); flag de modo; auth resolvendo `(usuário, tenant, papel)`; migração pro tenant 0; testes de isolamento.

**Fora do P1:** signup, convites, seletor de workspace, OIDC multi-tenant/por-tenant (P2); resolução `Host→tenant`, verificação de domínio, slugs por domínio, wellknown por tenant (P3); Sheets como extensão por tenant, ClickHouse por tenant (P4).

## Testes

- **Isolamento (o teste-chave):** dois tenants, cada um cria links/tokens/pixels; asserir que listagens/lookups/agregações de um **nunca** enxergam o outro, em **ambos** os backends. Um teste de "scan sem filtro" que falharia se algum método esquecesse o prefixo/WHERE.
- **Paridade OSS:** com `QUARK_MULTI_TENANT` desligado, a suíte atual (lib 212, api_it 87) passa idêntica — o tenant 0 não muda comportamento observável.
- **RLS (Postgres gated):** com RLS ligado, uma query sem `SET LOCAL app.tenant_id` retorna vazio (fail-closed), e com o tenant errado não vaza.
- **Migração:** dados pré-tenancy viram do tenant 0 e continuam resolvíveis.
- **Auth:** contrato de status do `admin_guard` inalterado; token/sessão resolvem o tenant certo.

## Riscos (os 5 mais duros, da auditoria)

1. **Namespace de código** — mitigado pela decisão de mantê-lo global (Feistel intocado); `tenant_id` é posse, não partição. Documentar explícito no código.
2. **Threading do `tenant_id` no `Store` + 2 backends** — mitigado pelo handle escopado (some a classe de bug de call-site) + teste de isolamento que cobre todo método tenant-owned.
3. **Rebuild da auth** — preservar o contrato exato do `admin_guard` (401/403/404/429) e o fallback "encurtador aberto"; coberto por testes de contrato.
4. **Singleton do `sheets_connection`** — redesenho da tabela + do loop de sync; a parte por-tenant do sync fica no P4, mas o schema `(tenant_id)` entra no P1.
5. **`Host→tenant` / wellknown / public_host** — **fora do P1** (P3); no P1 o `public_host` único e o tenant-por-credencial bastam.

## Sub-divisão provável (pro plano)

O P1 tende a virar dois planos de implementação:
- **P1a** — modelo de dados + `Store`/backends + migração + testes de isolamento (o grosso).
- **P1b** — binding da auth (`admin_guard`, tokens/sessões carregando tenant) + flag de modo + paridade OSS.
