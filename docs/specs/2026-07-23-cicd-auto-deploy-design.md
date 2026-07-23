# CI/CD: auto-deploy do front e do back (LUC-92)

Design do auto-deploy contínuo do quark, resolvendo a dívida achada ao publicar
a Fase 3 da LUC-87: o front (Cloudflare Pages) não deployava sozinho e o build
podia sair sem a base da API embutida.

## Decisões (alinhadas com o Lucas)

- **Front:** Cloudflare Pages com **integração Git** no projeto existente
  `quark-panel` (→ app.quarkus.com.br). O CF builda e deploya sozinho no push da
  `main`, e gera preview por PR. As envs de build ficam no dashboard do CF.
  (O CF passou a permitir conectar Git a um projeto direct-upload existente, então
  não foi preciso criar projeto novo nem mover o domínio.)
- **Back:** GitHub Actions com `flyctl deploy` no push da `main`, gated nos testes
  do CI (`needs: check`).

## Front (Cloudflare Pages Git integration)

Configurado no dashboard do CF (passo do Lucas, feito):
- Repo conectado: `quark`, production branch `main`.
- Root directory: `web`. Build command: `npm run build`. Output: `dist`.
- Env vars (Production + Preview): `VITE_API_BASE_URL=https://backend.quarkus.com.br`,
  `NODE_VERSION=20`.

Reforços no repo (rede de segurança, pra o build nunca depender só do dashboard):
- `web/.env.production` com `VITE_API_BASE_URL=https://backend.quarkus.com.br`. O
  valor é público (vai embutido no JS do cliente de qualquer jeito), não é
  segredo. Isso garante que qualquer `npm run build` (CF, CI, local ou manual)
  sai apontando pro backend certo, mesmo se a env do dashboard for esquecida —
  foi exatamente esse esquecimento que quebrou o login no deploy manual da Fase 3.
- `web/.node-version` com `20`, pro build do CF usar o mesmo Node do CI.

Resultado: push na `main` → CF builda+deploya o painel; PR → preview URL.

## Back (GitHub Actions + flyctl)

Novo job `deploy-backend` no `.github/workflows/ci.yml`:
- Dispara só no `push` da `main` (`if: github.event_name == 'push' && github.ref ==
  'refs/heads/main'`).
- `needs: check` — só deploya se o job de testes/clippy/fmt/build do backend
  passar. Não fica preso ao job `web` (o front tem o pipeline próprio no CF).
- `concurrency: deploy-backend` com `cancel-in-progress: false` — nunca dois
  deploys do back ao mesmo tempo.
- Usa `superfly/flyctl-actions/setup-flyctl` (pinado numa versão) + `flyctl deploy
  --remote-only -a quark-prod`.
- Autentica com o secret `FLY_API_TOKEN` (deploy token do `quark-prod`, criado com
  `flyctl tokens create deploy -a quark-prod -x 8760h`, guardado nos GitHub
  Secrets do repo).

O job `check` (testes do backend com Postgres/Valkey/ClickHouse de serviço) e o
job `web` (lint/typecheck/test/build do front) continuam como estão, rodando em
PR e na `main` como gate.

## Constraints do projeto

- `cargo`/`npm` como já são (o CI já tem os toolchains).
- Não versionar segredos: `FLY_API_TOKEN` fica nos GitHub Secrets; `VITE_API_BASE_URL`
  é público e pode ir no `web/.env.production`.
- `src/codec.rs`/`src/permute.rs` intocados (não fazem parte desta task).

## Fora de escopo

- Deploy do front via GitHub Actions/wrangler (descartado: o CF Git integration
  já cobre, com preview grátis).
- Migração de secrets do backend (já estão no Fly).
- Rollback automático / blue-green (o Fly já faz health check no deploy e mantém a
  versão anterior se a nova não passa).

## Passos manuais (uma vez, do Lucas)

1. `flyctl tokens create deploy -a quark-prod -x 8760h` → colar a saída no GitHub
   Secret `FLY_API_TOKEN` (repo `quark` → Settings → Secrets and variables →
   Actions).
2. CF Pages: conectar Git no `quark-panel`, setar root/build/output + as env vars.
   (Feito.)

## Verificação

- Push numa branch → CF gera preview do front (confirma que o build do CF passa
  com root=web + envs).
- Merge na `main` → CF publica o front em app.quarkus.com.br E o job
  `deploy-backend` roda o `flyctl deploy` (confirma o token + o gate).
- `GET /admin/me` responde do backend novo; `app.quarkus.com.br` serve o bundle
  novo com a base da API certa (login mostra SSO, não o token de admin).
