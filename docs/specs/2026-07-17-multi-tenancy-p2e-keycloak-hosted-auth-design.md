# P2e — Auth hospedado: Keycloak realm por tenant (modelo B) (cloud-only)

**Status:** design (aguardando aprovação antes de implementar). Decisão do usuário: OIDC por-tenant = **quark hospeda o Keycloak** (LUC-55). Camada ADITIVA sobre o P2d-A (merge `f4a9b51`), que fica intocado. Base `main @ af6068d`. Cloud-only.

## Objetivo

O quark controla um Keycloak. Ao criar um tenant, o quark **auto-provisiona um realm** (`<slug>`) via Admin API — client, grupos (admin/readonly), mapper de `groups` — e auto-popula o `oidc_config` do tenant (issuer **derivado** `<base>/realms/<slug>`). O login (`/admin/login?org=<slug>`), callback, cache e `claim_role` são os do P2d-A, **sem mudança**. Os convites (P2c) passam a **provisionar o usuário no realm** com o grupo do papel; a membership do quark nasce no primeiro login (claim de grupo).

## Contexto (da pesquisa)

- **P2d-A é source-agnostic:** `TenantOidcConfig` é struct plana; `oidc_login`/`oidc_callback`/`OidcRuntime::from_config`/`claim_role`/`TenantOidcCache` não sabem se a config foi colada ou provisionada. **Modelo B = mudança só de escrita** que termina chamando `put_oidc_config`.
- **Template de realm ≈ `e2e/keycloak/quark-realm.json`:** realm (enabled, sslRequired) + 1 client (`quark`, `standardFlow`, redirect = `/admin/callback` do quark) + 2 grupos (`quark-admins`/`quark-readers`) + 1 `oidc-group-membership-mapper` (claim `groups`, `full.path=false`) — exatamente o que o `claim_role` lê.
- **HTTP client:** `reqwest` já é dep; espelhar `src/sheets/client.rs` (trait `#[async_trait]` mockável + struct com `reqwest::Client` + token via form POST). Admin token = `client_credentials` contra o realm `master` (service account com `create-realm`/`manage-realm`), short-lived (refetch por chamada/401).
- **Hook:** `admin_tenants_create` (`src/api.rs`) já tem o padrão best-effort (seed de subdomínio) + backfill no boot (`main.rs`). Sem tenant-delete hoje (teardown de realm = follow-up).
- **Convites P2c** (mergeado): tabela `invites` + create/accept; hoje accept cria membership. No modelo B a membership vem do login.

## Decisões travadas (usuário + meus defaults)

1. **Convites integrados** (usuário): o convite provisiona o usuário no realm Keycloak do tenant com o grupo do papel; membership auto no 1º login (claim). Uma UI só.
2. **Client público + PKCE** (default meu): sem client_secret armazenado (PKCE já é sempre enviado). Elimina o secret-at-rest.
3. **Primeiro acesso mediado pelo quark** (default meu; sem infra de e-mail): o link do convite abre uma página do quark → a pessoa define a senha → quark seta via Admin API (`reset-password`). (Alternativa: Keycloak "execute actions email" se houver SMTP — opt-in.)
4. **Provisão best-effort + backfill no boot** (default): falha na provisão não quebra o create do tenant; sweep recria; 409 realm-existe = ok.
5. **Template embutido** (default), configurável depois via `QUARK_KEYCLOAK_REALM_TEMPLATE`.
6. **Keycloak roda em prod = infra do usuário** (Fly, análogo ao ClickHouse) — pré-requisito, não shippa com o quark.

## Arquitetura

### Config + client Admin
- Env: `QUARK_KEYCLOAK_BASE_URL` (base + issuer derivado; deve bater com o `iss` que o Keycloak emite via `KC_HOSTNAME`), `QUARK_KEYCLOAK_ADMIN_CLIENT_ID`/`_SECRET` (service account). `AppState.keycloak: Option<KeycloakRuntime>` (base + reqwest client + cache do admin token), construído no boot, `None` quando não setado (opt-in, como o resto).
- `KeycloakAdmin` trait (`#[async_trait]`, mockável) espelhando `SheetsApi`: `admin_token()`, `create_realm(slug)`, `create_client(slug, redirect)`, `create_group(slug, name)`, `add_group_mapper(...)`, `create_user(slug, email, group)`, `set_user_password(slug, user, pw, temporary)`. Concrete `HttpKeycloakAdmin` (reqwest).

### Provisão no create do tenant
`admin_tenants_create`: após criar tenant+membership (Owner) + seed subdomínio, SE `st.keycloak` setado: provisiona o realm (idempotente), o client (público+PKCE), os 2 grupos + mapper; cria o usuário **Owner** no realm (e-mail do criador, grupo `quark-admins`); `put_oidc_config(tenant, { issuer: <base>/realms/<slug>, client_id: "quark", client_secret: "", scopes, admin_claim:"groups", admin_value:"quark-admins", readonly_value:"quark-readers", required_value: Some("quark-...") })`. Best-effort + backfill no boot (detecta tenants sem `oidc_config`).

### Convite → provisão no realm
`admin_invites_create` (P2c) ganha, no modelo B: além de gravar o convite, **provisiona o usuário no realm** do tenant (Admin API `create_user` com o grupo do papel: Admin→quark-admins, Member/Viewer→quark-readers ou um grupo por papel) + gera o link de 1º acesso do quark. O `accept` deixa de criar membership (isso vem do login); vira "definir senha" (página do quark → `set_user_password`). A membership nasce no 1º login `?org=slug` (claim). (Se `st.keycloak` não setado = modelo A puro, comportamento atual do P2c.)

### Login/callback
Inalterado (P2d-A). `?org=slug` → `get_oidc_config_bare` (agora auto-provisionado) → runtime → valida → membership do claim.

## Escopo
**Dentro:** `KeycloakAdmin` trait + client HTTP; config + `AppState.keycloak`; provisão de realm/client/grupos/mapper/Owner no create + backfill; integração do convite (provisiona user no realm + 1º acesso mediado); auto-popular `oidc_config`. Reusa login/callback/claim do P2d-A. Cloud-only, opt-in (`QUARK_KEYCLOAK_BASE_URL`).
**Fora:** deploy do Keycloak (infra do usuário); teardown de realm no delete de tenant (não há delete hoje); customização do realm pelo tenant; proxy completo de CRUD de usuários no painel (o console do Keycloak cobre o resto); SMTP/execute-actions-email (opt-in futuro).

## Testes
- **Provisão** (mock `KeycloakAdmin`): create do tenant chama create_realm/client/groups/mapper/owner na ordem + `put_oidc_config` com issuer derivado; idempotente (409 realm = ok); best-effort (falha não quebra o 201); backfill recria.
- **Convite integrado** (mock): create do convite provisiona o user no realm com o grupo do papel; 1º acesso seta senha via Admin API; sem Keycloak (`st.keycloak=None`) = comportamento P2c-A (accept cria membership).
- **Login E2E** (Keycloak real do e2e): já coberto pro global; estender com um realm provisionado (encaixa no LUC-49).
- **Paridade:** `st.keycloak` não setado → nada de Keycloak, P2d-A/P2c-A intactos; OSS intocado.
- Postgres gated; unit tests network-free via o mock trait (sem Keycloak real, igual ao padrão do Sheets).

## Riscos
1. **Issuer mismatch** (`<base>/realms/<slug>` vs o `iss` real do Keycloak, que depende de `KC_HOSTNAME`) quebra o `verify_id_token` silenciosamente. Mitigação: documentar o `KC_HOSTNAME` obrigatório; teste que o issuer derivado bate com o discovery.
2. **Provisão parcial** (realm ok, client falha) deixa tenant meio-provisionado. Mitigação: backfill idempotente detecta e completa; ordem determinística; 409 = já-existe.
3. **Admin token** short-lived (60s) — refetch por chamada/401.
4. **Convite acopla ao Keycloak** — se `st.keycloak` cair, o create do convite degrada (best-effort + retry) em vez de falhar; documentar.
5. **Chicken-and-egg do 1º acesso:** o convidado não tem user no realm até ser provisionado → por isso o convite PROVISIONA no create (não no accept), e o accept só seta senha.
6. **Infra:** Keycloak em prod (Fly) + `KC_HOSTNAME`/`QUARK_KEYCLOAK_BASE_URL` consistentes — pré-requisito operacional.
