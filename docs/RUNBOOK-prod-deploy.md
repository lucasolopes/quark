# Production deploy runbook (quark cloud, São Paulo)

How the quark cloud is deployed today: the apps, the topology, the secrets, and
the non-obvious wiring that split-domain SSO needs. Everything runs in Fly's
`gru` region (São Paulo). Secrets are set with `fly secrets set` / `wrangler`,
never committed. This runbook lists secret names and shapes, never values.

## Topology

| Piece | Where | Purpose |
|---|---|---|
| Backend `quark-prod` | Fly app, `gru` | The quark binary (`Dockerfile`). Serves the API + redirects at `backend.quarkus.com.br`. Cloud mode on. |
| Panel `quark-panel` | Cloudflare Pages | The React SPA (`web/`), served at `app.quarkus.com.br`. Direct-upload (no Git integration). |
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
`QUARK_ACCESS_LOG`.

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

## Panel (Cloudflare Pages `quark-panel`)

The SPA reads the API base from `VITE_API_BASE_URL` at build time. Build against
prod and deploy the static output:

```
cd web
VITE_API_BASE_URL=https://backend.quarkus.com.br npm run build
npx wrangler pages deploy dist --project-name quark-panel
```

`wrangler pages deploy` needs a token with Pages Edit (`npx wrangler login` and
authorize Pages if the stored token is read-only). `app.quarkus.com.br` is mapped
to the project as a custom domain.

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

- Redeploy backend: `fly deploy -a quark-prod`.
- Redeploy panel: rebuild (`VITE_API_BASE_URL=...`) + `wrangler pages deploy`.
- Rotate a secret: `fly secrets set NAME=value -a quark-prod` (rolling restart).
- Keycloak admin console: `https://auth.quarkus.com.br/admin` (bootstrap admin).
- Tail logs: `fly logs -a <app>`.
- Kill the Redis bill if it ever comes back: single-node needs no Valkey; unset
  `QUARK_VALKEY_URL` and destroy the resource.
