# LUC-16 — Login com Google

Data: 2026-07-18
Issue: LUC-16

## Diagnóstico (investigação)

O fluxo OIDC do quark já é genérico e **já funciona com o Google** sem código
novo de auth:
- `OidcConfig::from_env` + discovery + PKCE + verificação de id_token
  (`src/oidc.rs`) rodam contra qualquer issuer OIDC padrão, inclusive
  `https://accounts.google.com`.
- A autorização é por claim: `claim_contains` (`src/oidc.rs:332`) casa tanto
  claim **string** quanto **array**. O Google não emite `groups`, mas emite
  `email` (string) e, em Google Workspace, `hd` (hosted domain, string). Então:
  - `admin_claim=email` + `admin_value=<seu-email>` → 1 admin específico.
  - `admin_claim=hd` + `admin_value=<domínio-workspace>` → todos do Workspace.
  Nenhuma mudança no gate de authz é necessária.
- O painel já tem o botão OIDC genérico (`Login.tsx`, `login.oidcButton`,
  dispara `oidcLoginUrl()`). Ele já dispara o fluxo — só não é rotulado
  "Google".

Ou seja, o LUC-16 é majoritariamente **documentação + verificação**, mais um
rótulo de botão configurável pra cumprir o AC "botão Login com Google".

## Escopo

1. **Rótulo de botão configurável** (genérico, não hardcode Google):
   - `OidcConfig`: novo campo `button_label: Option<String>` lido de
     `QUARK_OIDC_BUTTON_LABEL` (None quando não setado).
   - `/admin/me` (`src/api.rs`): expõe `oidc_button_label` (a string, ou
     ausente/None) junto de `oidc_enabled`.
   - `Login.tsx`: o botão OIDC compartilhado (o de `login.oidcButton`, ~L206)
     usa `me.oidc_button_label` quando presente, senão o label i18n atual.
     (Os botões de `?org=` e de descoberta por e-mail são caminhos
     multi-tenant e ficam como estão.)
   - Assim o operador que configura Google seta
     `QUARK_OIDC_BUTTON_LABEL="Login com Google"`.
2. **Docs** (`docs/OIDC-LOGIN.md` + `.PT_BR.md`): nova seção "Google" com:
   - criar o OAuth client no Google Cloud Console (Authorized redirect URI =
     `QUARK_OIDC_REDIRECT_URL`; scopes `openid email profile`);
   - config de env (`QUARK_OIDC_ISSUER=https://accounts.google.com`,
     `_CLIENT_ID`, `_CLIENT_SECRET`, `_REDIRECT_URL`);
   - a estratégia de autorização do Google (sem `groups`): `admin_claim=email`
     (admin único) ou `admin_claim=hd` + `admin_value=<domínio>` (Workspace),
     deixando claro o default-closed;
   - menção ao `QUARK_OIDC_BUTTON_LABEL` opcional.
   Sem em-dash; EN/PT_BR sincronizados.
3. **Teste** (`src/oidc.rs` unit tests): claims no formato Google
   (`{"email":"me@acme.com","hd":"acme.com"}`) →
   - `map_scopes` com `admin_claim=hd, admin_value=acme.com` → `[Scope::Full]`;
   - `map_scopes` com `admin_claim=email, admin_value=me@acme.com` →
     `[Scope::Full]`;
   - `map_scopes` com o default `admin_claim=groups` (ausente) → `[]`
     (default-closed preservado).

## Fora de escopo

- Multi-tenant (config OIDC por tenant já existe em LUC-25; este é o caminho
  global/OSS single-tenant).
- SDK do Google / verificação especial: o fluxo OIDC padrão basta.

## Critérios de aceite

- [ ] `QUARK_OIDC_ISSUER=https://accounts.google.com` + client id/secret loga
      ponta a ponta pelo fluxo existente (coberto por doc + a verificação de
      que os claims do Google autorizam via `email`/`hd`).
- [ ] Botão de login no painel pode dizer "Login com Google" via
      `QUARK_OIDC_BUTTON_LABEL` (fallback pro label atual).
- [ ] Estratégia de authz do Google documentada (`email`/`hd`) e testada.
- [ ] Suíte Rust + web + clippy + fmt + tsc verdes.
