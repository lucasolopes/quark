# Keycloak-hosted auth: production runbook (multi-tenancy P2e)

This covers what an operator needs to stand up Keycloak as the identity provider for quark cloud: deploying it, wiring the admin service account, setting env vars, and configuring SMTP for the invite emails Keycloak sends on quark's behalf.

Real end-to-end validation against a live Keycloak instance is deferred to LUC-49. Everything below is verified against `MockKeycloakAdmin`/`FailingKeycloakAdmin` in `tests/workspace_it.rs` and `tests/invites_it.rs`, not against a running Keycloak server.

## What gets provisioned, and when

Every tenant gets its own Keycloak realm, named after the tenant's slug. `admin_tenants_create` (`src/api.rs`) provisions it inline, best-effort: if any step fails, the tenant is still created and the boot backfill (`backfill_keycloak_provisioning`, run once at startup) retries any tenant missing an `oidc_config` row.

```mermaid
sequenceDiagram
    participant U as User
    participant Q as quark API
    participant K as Keycloak
    participant S as Store (Postgres)

    U->>Q: POST /admin/tenants
    Q->>S: put_tenant + put_membership (Owner)
    Q->>K: ensure_realm(slug)
    Q->>K: ensure_client(slug, redirect_uri)
    Q->>K: ensure_groups_and_mapper(slug)
    Q->>K: ensure_user(slug, owner_email, quark-admins)
    K-->>Q: kc_user_id
    Q->>K: send_set_password_email(slug, kc_user_id)
    K-->>U: "set your password" email
    Q->>S: put_oidc_config(issuer, client_id, empty secret)
    Q-->>U: 200 tenant

    Note over U,K: First login
    U->>K: sets password, authenticates
    K-->>U: redirect with code
    U->>Q: GET /admin/callback?code=...
    Q->>K: verify_id_token (checks iss == oidc_config.issuer)
    Q->>S: put_membership (role from quark-admins/quark-readers group claim)
    Q-->>U: session cookie
```

Each step is idempotent on the real client (a `409` from Keycloak counts as success), so replaying provisioning for an already-provisioned tenant is safe.

## Prerequisites before turning this on

### 1. Deploy Keycloak

Run Keycloak somewhere quark's backend can reach it. The existing infra docs (`docs/DEPLOY.md`, `docs/DEPLOY-MULTIREGION.md`) cover Fly as the reference platform; a Keycloak deployment there is a normal container app with a Postgres backing store, same shape as any other Fly service.

### 2. Match `KC_HOSTNAME` to `QUARK_KEYCLOAK_BASE_URL`

This is the prerequisite most likely to bite. Keycloak stamps its own `KC_HOSTNAME` into the `iss` (issuer) claim of every token it signs. `verify_id_token` (`src/oidc.rs`) rejects any token whose issuer doesn't match `oidc_config.issuer`, which quark derives from `QUARK_KEYCLOAK_BASE_URL` as `{base}/realms/{slug}`.

If `KC_HOSTNAME` and `QUARK_KEYCLOAK_BASE_URL` disagree, even by scheme (`http` vs `https`) or a trailing detail, every login fails with an issuer mismatch. Set both to the same externally-reachable URL, protocol included.

### 3. Create the admin service-account client

quark's admin API calls (create realm, create client, create groups, create users, trigger the set-password email) run as a Keycloak service account, not as a human admin. In the target Keycloak instance:

- Create a confidential client (client credentials grant).
- Grant it `create-realm` on the master realm's `realm-management` role, plus `manage-realm`, `manage-clients`, `manage-users`, and `manage-users` on realms it provisions (the `admin-cli`/`master-realm` service-account role model in Keycloak's docs covers the exact role assignment).
- Note the client ID and secret; they become `QUARK_KEYCLOAK_ADMIN_CLIENT_ID` and `QUARK_KEYCLOAK_ADMIN_CLIENT_SECRET`.

### 4. Set the env vars

| Variable | Value |
|---|---|
| `QUARK_KEYCLOAK_BASE_URL` | The externally-reachable Keycloak URL, matching `KC_HOSTNAME` exactly |
| `QUARK_KEYCLOAK_ADMIN_CLIENT_ID` | The service-account client ID from step 3 |
| `QUARK_KEYCLOAK_ADMIN_CLIENT_SECRET` | The service-account client secret from step 3 |
| `QUARK_KEYCLOAK_SMTP_HOST` | SMTP relay host (see below) |
| `QUARK_KEYCLOAK_SMTP_PORT` | SMTP relay port |
| `QUARK_KEYCLOAK_SMTP_USER` | SMTP auth user |
| `QUARK_KEYCLOAK_SMTP_PASSWORD` | SMTP auth password |
| `QUARK_KEYCLOAK_SMTP_FROM` | From-address for invite/set-password emails |
| `QUARK_KEYCLOAK_SMTP_STARTTLS` | `true`/`false` |

Leaving `QUARK_KEYCLOAK_BASE_URL` unset keeps Keycloak disabled entirely (`src/main.rs` logs `keycloak admin: disabled`); tenant creation and invites then behave exactly as they did before P2e (model A, no Keycloak calls at all).

### 5. SMTP for the invite/set-password emails

Keycloak, not quark, sends the "set your password" email when a tenant owner or invited member is provisioned (`ensure_user` + `send_set_password_email` in `src/keycloak/client.rs`). This SMTP config is Keycloak's realm-level SMTP settings, applied through `ensure_realm`.

Two providers work out of the box:

**SendGrid**

| Setting | Value |
|---|---|
| `QUARK_KEYCLOAK_SMTP_HOST` | `smtp.sendgrid.net` |
| `QUARK_KEYCLOAK_SMTP_PORT` | `587` |
| `QUARK_KEYCLOAK_SMTP_USER` | `apikey` (literal string, not your SendGrid username) |
| `QUARK_KEYCLOAK_SMTP_PASSWORD` | your SendGrid API key |
| `QUARK_KEYCLOAK_SMTP_STARTTLS` | `true` |

**Resend**

| Setting | Value |
|---|---|
| `QUARK_KEYCLOAK_SMTP_HOST` | `smtp.resend.com` |
| `QUARK_KEYCLOAK_SMTP_PORT` | `465` |
| `QUARK_KEYCLOAK_SMTP_USER` | `resend` (literal string) |
| `QUARK_KEYCLOAK_SMTP_PASSWORD` | your Resend API key |
| `QUARK_KEYCLOAK_SMTP_STARTTLS` | `false` (465 is implicit TLS, not STARTTLS) |

Both providers use the API key as the password; there's no separate SMTP credential to generate.

## Realm branding / login theme

Each tenant gets its own Keycloak realm, and each realm serves its own login page. The theme for that page (logo, colors, CSS) is a Keycloak theme: a set of files (`.ftl` templates, CSS, images) deployed on the Keycloak server itself, under `providers`/`themes` (or the equivalent volume for the deployment). It is not quark code; quark only references the theme by name through the realm's `loginTheme` attribute.

That means changing login branding always takes two steps, in this order: deploy the theme on the Keycloak server first (outside quark), then point the realm(s) at it. Pointing a realm at a theme that isn't deployed on the server yet breaks that realm's login page (Keycloak errors when it tries to render it).

### Global path: one shared theme for every realm

For the simple case, a single quark-branded theme applied to every tenant, there is an opt-in hook: `QUARK_KEYCLOAK_LOGIN_THEME`.

1. Deploy the theme (e.g. `quark-branded`) on Keycloak, on every node/instance that serves the login page.
2. Set `QUARK_KEYCLOAK_LOGIN_THEME=quark-branded` in quark's environment.
3. Every realm `ensure_realm` provisions from then on (a new tenant, or a backfill run for an existing one) is created with `"loginTheme": "quark-branded"` in the realm-create body.

Without this env var (the default), `ensure_realm` leaves the `loginTheme` key out of the body entirely, and the realm keeps Keycloak's stock login theme. That is deliberate: since the theme must already be deployed, the hook is opt-in rather than automatic.

Note: `ensure_realm` is a create call (`POST /admin/realms`); an already-existing realm (409) is not resent, so turning on `QUARK_KEYCLOAK_LOGIN_THEME` after realms already exist does not retroactively change their theme. For realms already provisioned, use the per-tenant path below.

### Per-tenant path: real per-company branding

When each tenant needs its own branding instead of one shared theme, the model is a theme per tenant on Keycloak (e.g. `quark-<slug>`), applied individually to each realm through the Admin API. This is manual (or externally scripted) config-ops, not part of quark's automatic provisioning:

1. Deploy the `quark-<slug>` theme on Keycloak (same mechanism as the global path, one theme per tenant instead of one shared theme).
2. Apply the theme to the tenant's realm through the Admin API:

   ```
   PUT /admin/realms/<slug>
   Content-Type: application/json
   Authorization: Bearer <admin token>

   {"loginTheme": "quark-<slug>"}
   ```

3. Confirm visually (the realm's login page) before considering it done.

As with the global path, setting `loginTheme` to a theme that doesn't exist on the server breaks that realm's login. Always deploy the theme first, point the realm at it second.

### Dependency

Both paths require a live Keycloak in production (see "Prerequisites" above); neither has been validated against a real instance yet, same caveat as "Deferred: real end-to-end validation" at the end of this runbook.

## Known gap: backfilled tenants and the Owner

`backfill_keycloak_provisioning` calls `provision_tenant_keycloak` with `owner_user_id: None` for every tenant it provisions on boot. That's deliberate: the backfill has no request context to know who should become the Keycloak-side Owner, so it provisions the realm, client, and groups but skips `ensure_user`/`send_set_password_email` entirely.

Practically: a tenant created before Keycloak was configured gets its realm and groups set up by the backfill, but its Owner has no Keycloak user yet and won't be able to log in through Keycloak until someone is added to `quark-admins` by hand (through the Keycloak admin console, or a follow-up manual `ensure_user` call). This is tracked as LUC-56. Until it's fixed, treat any pre-existing tenant as needing a manual first-user step after enabling Keycloak.

## Deferred: real end-to-end validation

Every test backing this runbook uses `MockKeycloakAdmin` or a hand-written failing mock; no test in this repo talks to an actual Keycloak server. Before relying on this in production, run the full flow against a real instance at least once: create a tenant, receive the set-password email, log in, and confirm the group claim maps to the right role. That validation pass is LUC-49.
