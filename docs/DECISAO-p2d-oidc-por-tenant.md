# Decisão pendente — P2d OIDC por-tenant (LUC-25): como o login resolve o tenant?

**Status:** aguardando decisão do usuário (levantada 2026-07-17 madrugada, /loop). É o último pedaço do P2 (LUC-7). Não implementei às cegas porque envolve (a) um fork de UX/produto de login, (b) auth (historicamente bug-prone aqui — o OIDC do P1b/P2b levou 20 achados em 3 rodadas de review) e (c) postura de secret-at-rest. Melhor você decidir e eu implementar em cima.

## O problema (a "crux")

Hoje o OIDC é 100% global: um `issuer`/`client_id`/`client_secret` vindos de env (`OidcConfig::from_env`, `src/oidc.rs:38`), um runtime único em `AppState.oidc`. O `/admin/login` (`src/api.rs:1547`) começa o fluxo **sem nenhum contexto de tenant** — só tem o `HeaderMap`; o frontend chama `oidcLoginUrl()` = `${BASE}/admin/login` sem argumento (`web/src/lib/api.ts:49`).

P2d quer que **cada tenant enterprise plugue o próprio IdP**. O ponto difícil: o login precisa saber **qual config de tenant usar ANTES do usuário se autenticar**. Como o quark descobre o tenant nesse ponto?

## Opções (com o que cada uma custa no código atual)

**1) Slug na URL** (`/login?org=acme` → `/admin/login?org=acme`) — **RECOMENDADA**
- Menor diff: `oidc_login` lê `?org=`, busca `oidc_configs` pelo slug do tenant (reusa `tenants.slug` único), monta o runtime OIDC daquele tenant. O tenant vai assinado no cookie `qk_login` (estender `sign_login_state`/`verify_login_state`, `oidc.rs:457`) pra o `/admin/callback` saber contra qual config validar.
- Coexiste trivial com o env global: sem `?org` = login de plataforma (operador/OSS) no `st.oidc`; com `?org` = config do tenant. História de coexistência mais limpa.
- Custo UX: o usuário precisa saber/bookmarkar o slug da org (padrão enterprise tipo "entre com sua organização"). Frontend: `Login.tsx` ganha um campo de org ou a rota carrega o slug.

**2) Host / domínio custom (amarra no P3)**
- Reusa `host_router.resolve(host)` (já mapeia Host→tenant pro redirect). Melhor UX (login "só funciona" em `admin.acme.com`, zero input), mas o **maior lift**: hoje `domains` é pra redirect de link, não pro origin do painel/API; precisaria de um conceito de "host de admin por tenant" + mudanças de CORS/cookie-domain. **E depende da decisão pendente do host compartilhado** (`DECISAO-host-compartilhado-P3.md`).

**3) Lookup por domínio de e-mail** (usuário digita e-mail → mapeia `acme.com`→tenant→IdP)
- Precisa de tela pré-login (digite e-mail), tabela nova e-mail-domínio→tenant, e trata ambiguidade (vários tenants no mesmo domínio) / e-mails pessoais (gmail). Maior superfície nova, sem precedente no código.

**Minha recomendação: opção 1 (slug).** É o menor diff contra a arquitetura atual, coexiste limpo com o OIDC global (que fica como login de plataforma/OSS), e não fica preso na decisão do host compartilhado. Se você quiser o UX de "login no domínio próprio", é a opção 2 — mas aí é bem mais código e depende do P3.

## Outras decisões embutidas (preciso do seu aval)

- **`client_secret` at rest:** o único precedente do quark (`sheets_connection.blob`, refresh token do Google) é **plaintext em JSONB**, isolado por tenant no app-level, RLS ENABLE-sem-FORCE. Se o P2d seguir o precedente, os `client_secret` dos IdPs dos tenants ficam plaintext no Postgres. Opções: (a) seguir o precedente (plaintext, mesma postura do Sheets) e anotar como hardening futuro; (b) cifrar a coluna (envelope/KMS) — não há prior art no código, é trabalho novo. Recomendo (a) pra este item + issue de hardening separada, mas é secret sensível, quero seu OK.
- **Coexistência global vs por-tenant:** OSS fica exatamente como hoje (env global único, intocado). Cloud adiciona lookup por-tenant **além** do env global (que vira fallback "staff do quark / default"). O `admin_guard` e o `ensure_user_and_membership` já ramificam em `st.multi_tenant`, então o encaixe é natural.
- **Claim de grupo → papel:** no cloud a autorização já vem de `role_scopes(membership)` a cada request (o `session.scopes` do claim é ignorado pós-login). Então o admin-group-claim do IdP do tenant deve mapear pro **`Role` da Membership** criada no accept/login (espelhando o OSS), não pra `Scope` direto.

## Plano quando você decidir (esboço, opção 1)

Fase P2d-backend: (T1) tabela `oidc_configs` (tenant_id, issuer, blob JSONB com client_id/secret/scopes/claim-map) em TENANT_OWNED+NOT_FORCED + CRUD admin (Owner/Admin setam o IdP do tenant); (T2) `OidcRuntime` vira cache keyed por tenant/issuer (discovery+JWKS por entrada, refresh lazy como hoje); (T3) `oidc_login` resolve o tenant pelo `?org=`, monta o runtime do tenant, assina o tenant no `qk_login`; (T4) `oidc_callback` recupera o tenant do cookie, valida contra a config certa, cria Membership com o papel do claim; (T5) OSS parity + sweep de segurança. Depois P2d-frontend (campo de org no Login). Gate PG não-superuser + review adversarial (auth = review no Opus).

## Enquanto isso

Não implementei P2d (nem a fundação — porque a tabela já commeteria a postura de secret-at-rest). P3-backend e P2c já mergeados. Vou (1) aplicar os fast-follows seguros do P2c/P3 (hygiene decidida) e (2) escrever a avaliação do P4 (`DECISAO-p4-*.md`) — o P4 também tem decisão embutida (provisionar ClickHouse = custo de infra). Depois disso o trabalho de feature fica esperando suas decisões da manhã.
