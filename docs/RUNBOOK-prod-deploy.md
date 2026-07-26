# Production deploy runbook (quark cloud, São Paulo)

How the quark cloud is deployed today: the apps, the topology, the secrets, and
the non-obvious wiring that split-domain SSO needs. Everything runs in Fly's
`gru` region (São Paulo). Secrets are set with `fly secrets set` / `wrangler`,
never committed. This runbook lists secret names and shapes, never values.

## Topology

| Piece | Where | Purpose |
|---|---|---|
| Backend `quark-prod` | Fly app, `gru` | The quark binary (`Dockerfile`). Serves the API + redirects at `backend.quarkus.com.br`. Cloud mode on. |
| Panel `quark-panel` | Cloudflare Pages | The React SPA (`web/`), served at `app.quarkus.com.br`. Git-connected for PR previews; production deploys come from the release workflow, not from a push. |
| Store | Fly Managed Postgres cluster `quark` | The main store (`QUARK_DATABASE_URL`, attached to `quark-prod`) and a separate `keycloak` database for Keycloak. |
| Cache/pubsub `quark-valkey` | Fly app, `gru` | Self-hosted Valkey on the private network. Optional for a single node; kept cheap (~256mb). |
| IdP `quark-keycloak` | Fly app, `gru` | Keycloak 26 at `auth.quarkus.com.br`. Per-tenant realm provisioning + the `quark-panel` login realm. Backing DB is the `keycloak` database in the MPG cluster. |

The panel and the API are on **different subdomains** (`app.` vs `backend.`).
That split is the source of most of the gotchas below.

## Backend (`quark-prod`)

Config lives in `fly.toml` (`app = "quark-prod"`, `primary_region = "gru"`).
Non-secret host config is in `fly.toml` `[env]`: `QUARK_PUBLIC_HOST = "go.quarkus.com.br"`
(shared short-link host + CNAME target for custom domains), `QUARK_ADMIN_HOST =
"backend.quarkus.com.br"` (the only host `/admin/*` answers on in cloud), and
`QUARK_TENANT_DOMAIN_SUFFIX = "quarkus.com.br"` (auto per-tenant subdomain base).
Everything sensitive is a secret.

Deploy the current `main`:

```
fly deploy -a quark-prod
```

### Secrets (names)

Core: `QUARK_KEY`, `QUARK_SIGNING_KEY`, `QUARK_DATABASE_URL` (MPG),
`QUARK_VALKEY_URL` (self-hosted Valkey, internal), `QUARK_CORS_ORIGINS`
(must include `https://app.quarkus.com.br` for the panel), `QUARK_RATELIMIT_PER_MIN`,
`QUARK_LOG_FORMAT` (`json` in production).

Cloud: `QUARK_MULTI_TENANT=1`, `QUARK_ENCRYPTION_KEY` (base64 32 bytes; secret-at-rest
for OIDC client secrets and Sheets refresh tokens; **back it up**, losing it means the
stored secrets cannot be decrypted).

OIDC login (global, for the panel): `QUARK_OIDC_ISSUER`
(`https://auth.quarkus.com.br/realms/quark-panel`), `QUARK_OIDC_CLIENT_ID`,
`QUARK_OIDC_CLIENT_SECRET`, `QUARK_OIDC_REDIRECT_URL`
(`https://backend.quarkus.com.br/admin/callback`), `QUARK_OIDC_ADMIN_CLAIM=groups`,
`QUARK_OIDC_ADMIN_VALUE=quark-admins`, `QUARK_OIDC_READONLY_VALUE=quark-readers`.

Split-domain redirects (see gotchas): `QUARK_OIDC_POST_LOGIN_URL=https://app.quarkus.com.br`,
`QUARK_OIDC_POST_LOGOUT_URL=https://app.quarkus.com.br/login`.

Keycloak provisioning: `QUARK_KEYCLOAK_BASE_URL=https://auth.quarkus.com.br`,
`QUARK_KEYCLOAK_ADMIN_CLIENT_ID`, `QUARK_KEYCLOAK_ADMIN_CLIENT_SECRET`, plus the
SMTP family for invite/set-password emails: `QUARK_KEYCLOAK_SMTP_HOST`,
`_PORT`, `_USER`, `_PASSWORD`, `_FROM`, `_STARTTLS`.

Break-glass: `QUARK_ADMIN_TOKEN` is intentionally **unset** in prod (SSO-only login;
the panel hides the token field when `admin_login_enabled` is false). Re-enable it in
an emergency (IdP down) with `fly secrets set QUARK_ADMIN_TOKEN=<token> -a quark-prod`.

## How production gets deployed

**A git tag is the only thing that deploys.** Pushing to `main` runs CI and
nothing else. Merging a pull request deploys nothing. This is deliberate: the
cloud and self-hosters run the exact same artifact, so a bug report that names a
version means the same code in both places.

Cutting a release:

```
scripts/release.sh 0.2.1        # bumps Cargo.toml, opens both CHANGELOG sections
                                # then commits and tags, but does not push
git push origin main && git push origin v0.2.1
```

The `Release` workflow then, in order: checks the tag against `Cargo.toml` and
the CHANGELOG, builds `linux/amd64` and `linux/arm64` on native runners, pushes
the multiarch image to `ghcr.io/lucasolopes/quark`, attests its provenance,
creates the GitHub Release, and only then deploys. The backend gets that exact
image **by digest** (`flyctl deploy --image`, so Fly never rebuilds), and the
panel is built here and pushed with `wrangler`. The last step polls
`backend.quarkus.com.br/health` until `X-Quark-Version` reports the new version,
so a deploy that silently rolled back fails the job instead of going green.

A prerelease tag (`v0.3.0-rc.1`) runs everything **except** the deploy. That is
the cheap way to rehearse a risky change to the pipeline itself.

### Rollback and manual redeploy

Actions > `Deploy manual` > Run workflow, with a version already published to
GHCR (`0.2.0`). It rebuilds nothing: it resolves that version's digest and puts
it back in production, panel included. Use it when prod is broken and you need
the previous version now, or when the image is fine but the deploy died halfway.

Both this workflow and the tag deploy share the `deploy-production` concurrency
group, so a manual rollback can never race a tag deploy.

### Panel (Cloudflare Pages `quark-panel`)

The SPA reads the API base from `VITE_API_BASE_URL` at build time. In CI that
comes from the `VITE_API_BASE_URL` repo variable, and the project name from
`CF_PAGES_PROJECT`. `app.quarkus.com.br` is mapped to the project as a custom
domain.

The project is Git-connected, but **automatic production branch deployments are
turned off** in the dashboard (Settings > Builds & deployments). That switch is
what makes the release workflow the only thing that touches production, and it
is the one piece of this whole model that lives outside the repository: nothing
here fails if somebody turns it back on, the panel just silently starts shipping
from `main` again. Worth re-checking whenever production behaves oddly.

Preview deployments for pull requests are a **separate switch on the same
screen** and are on: a pull request gets a Cloudflare Pages check and a preview
URL straight from the Git integration, with no workflow involved. The two
switches are easy to confuse, and turning the wrong one off costs you PR
previews without changing anything about production. If Pages checks stop
appearing on pull requests, that is the switch to look at.

A preview points at the *production* API, because `VITE_API_BASE_URL` is baked
in at build time. So a pull request that adds an endpoint can be previewed
visually, but not exercised end to end until its version ships.

To deploy the panel by hand, outside CI:

```
cd web
VITE_API_BASE_URL=https://backend.quarkus.com.br npm run build
npx wrangler pages deploy dist --project-name quark-panel --branch main
```

`wrangler pages deploy` needs a token with Pages Edit (`npx wrangler login` and
authorize Pages if the stored token is read-only).

## DNS (Cloudflare, `quarkus.com.br`)

- `backend.quarkus.com.br` -> Fly, DNS only (grey cloud) so Fly terminates TLS:
  `A 66.241.124.165`, `AAAA 2a09:8280:1::14e:87d5:0`, and the ownership record
  `TXT _fly-ownership.backend = app-wlqdm0e`.
- `app.quarkus.com.br` -> the Cloudflare Pages project.
- `auth.quarkus.com.br` -> Fly (`quark-keycloak`), DNS only: `A 66.241.124.30`,
  `AAAA 2a09:8280:1::151:73a4:0`, `TXT _fly-ownership.auth = app-ropxp68`. This is
  the Keycloak issuer host (`KC_HOSTNAME`), so it must match `QUARK_OIDC_ISSUER` and
  `QUARK_KEYCLOAK_BASE_URL` exactly.

Get the current values any time with `fly certs setup <hostname> -a <app>`.

## Link domains (short-link hosts)

Short links resolve on hosts separate from the panel (`app.`) and API (`backend.`):

- **Shared / default host:** `go.quarkus.com.br` (`QUARK_PUBLIC_HOST`). Where
  the default tenant's links live and the CNAME target shown to custom-domain
  customers. Covered by the `*.quarkus.com.br` wildcard (DNS + cert), so no
  dedicated DNS record is needed for it.
- **Per-tenant subdomain:** each workspace gets `<slug>.quarkus.com.br`
  automatically (`QUARK_TENANT_DOMAIN_SUFFIX = quarkus.com.br`). The boot
  backfill seeds a verified `domains` row per tenant; new links for that tenant
  bind to its subdomain.
- **Custom domains:** an Owner/Admin adds `go.acme.com` in the panel
  (`/domains`), publishes the shown DNS records (`CNAME go.acme.com →
  go.quarkus.com.br` and `TXT _quark-verify.go.acme.com → <token>`), and clicks
  Verify. Then issue TLS: `fly certs add go.acme.com -a quark-prod`.

### Wildcard DNS + cert (one-time)

- Cloudflare (DNS-only / grey): `A *.quarkus.com.br → 66.241.124.165`,
  `AAAA *.quarkus.com.br → 2a09:8280:1::14e:87d5:0`.
- `fly certs add "*.quarkus.com.br" -a quark-prod`, then add the DNS-01
  challenge it prints: `CNAME _acme-challenge.quarkus.com.br →
  quarkus.com.br.<id>.flydns.net.`. Check with `fly certs check "*.quarkus.com.br"`.

### Admin host gate

In cloud, `/admin/*` answers **only** on `QUARK_ADMIN_HOST`
(`backend.quarkus.com.br`); a request to `/admin/*` on any link domain (a
tenant subdomain or custom domain) gets a `404`. Link domains serve only the
public redirect path. Verify: `curl -sI https://<tenant-domain>/admin/me`
returns `404`, `https://backend.quarkus.com.br/admin/me` returns `200`.

## Keycloak (`quark-keycloak`)

Runs the `quay.io/keycloak/keycloak:26.0` image (config in a `fly.toml`, not in this
repo; env: `KC_DB=postgres`, `KC_DB_URL` pointing at the MPG `keycloak` database via
its pgbouncer endpoint with `?prepareThreshold=0`, `KC_DB_USERNAME`/`KC_DB_PASSWORD`,
`KC_HOSTNAME=https://auth.quarkus.com.br`, `KC_HTTP_ENABLED=true`,
`KC_PROXY_HEADERS=xforwarded`, `KC_HEALTH_ENABLED=true`, `KC_BOOTSTRAP_ADMIN_USERNAME`
+ `KC_BOOTSTRAP_ADMIN_PASSWORD` as secrets). VM `1024mb` (the JVM needs it).

The backing database is the `keycloak` database in the MPG cluster, wired with:

```
fly mpg attach <cluster-id> -a quark-keycloak -d keycloak --variable-name KC_ATTACH_URL
```

which prints a `postgresql://...pgbouncer...flympg.net/keycloak` connection string;
the pieces become `KC_DB_URL`/`KC_DB_USERNAME`/`KC_DB_PASSWORD`.

### Realms

- `quark-panel`: the login realm for the panel. Holds the confidential client
  `quark` (redirect `https://backend.quarkus.com.br/admin/callback`, a `groups`
  membership mapper, the `quark-admins`/`quark-readers` groups, and
  `post.logout.redirect.uris = https://app.quarkus.com.br/*`). This is the IdP the
  global `QUARK_OIDC_*` points at.
- One realm per tenant, created automatically by quark's provisioning (model B)
  when a workspace is created. The service account `quark-admin` (in the `master`
  realm, with the `admin` role) drives provisioning.

### Service account gotcha (fixed)

Creating a realm adds that realm's `<realm>-realm` management roles to the `master`
`admin` composite. A service-account token minted **before** the realm existed does
not carry them, so the immediate follow-up client/mapper call would `403`. quark's
Keycloak client now retries on `403` (not just `401`) with a fresh token
(`src/keycloak/client.rs`), which recovers transparently.

### First boot is slow

The app runs `kc.sh start` without `--optimized`, so it rebuilds the Quarkus
augmentation and runs migrations on every boot (several minutes on a shared CPU).
A follow-up is to ship a pre-built (`--optimized`) image for fast boots.

## Split-domain gotchas

Because the panel (`app.`) and the API (`backend.`) are different origins:

1. **CORS is credentialed.** `QUARK_CORS_ORIGINS` must list `https://app.quarkus.com.br`;
   the API allows credentials and a specific header list (`content-type`,
   `x-admin-token`, `x-quark-csrf`), never `*`. Cookies are host-only on `backend.`
   and are same-site to `app.` (both under `quarkus.com.br`), so they ride the panel's
   `fetch(..., {credentials})` calls.
2. **Post-login redirect.** After the OIDC callback, quark redirects to
   `QUARK_OIDC_POST_LOGIN_URL`. If unset it defaults to `/`, which on the API origin
   is the shortener's `POST /` (a `GET /` there is `405`). Set it to the panel:
   `https://app.quarkus.com.br`.
3. **Post-logout redirect.** RP-initiated logout sends the browser to Keycloak's
   `end_session_endpoint` and back to `QUARK_OIDC_POST_LOGOUT_URL`
   (`https://app.quarkus.com.br/login`). That URL must be allowed by the Keycloak
   client's `post.logout.redirect.uris`.

## Common operations

- Ship a change: `scripts/release.sh <versao>`, then push main and the tag.
- Roll back or redeploy: Actions > `Deploy manual`, with a version already in GHCR.
- Redeploy by hand, bypassing CI: `fly deploy --image ghcr.io/lucasolopes/quark:<versao> -a quark-prod`
  for the backend, rebuild plus `wrangler pages deploy` for the panel. Prefer the
  workflow: doing it by hand skips the version check against production.
- Rotate a secret: `fly secrets set NAME=value -a quark-prod` (rolling restart).
- Keycloak admin console: `https://auth.quarkus.com.br/admin` (bootstrap admin).
- Tail logs: `fly logs -a <app>`.
- Kill the Redis bill if it ever comes back: single-node needs no Valkey; unset
  `QUARK_VALKEY_URL` and destroy the resource.
