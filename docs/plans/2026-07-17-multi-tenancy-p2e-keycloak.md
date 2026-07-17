# Multi-tenancy P2e (Keycloak-hosted auth) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** quark auto-provisions a Keycloak realm per tenant (client + groups + group-claim mapper + SMTP + Owner user), auto-populates the tenant's `oidc_config` (derived issuer), and the P2c invite flow provisions the invited user in the realm + triggers Keycloak's set-password email. Reuses the merged P2d-A login/callback unchanged. Cloud-only, opt-in (`QUARK_KEYCLOAK_BASE_URL`).

**Architecture:** A mockable `KeycloakAdmin` trait (`reqwest`, mirroring `src/sheets/client.rs`) wraps the Keycloak Admin API. Realm provisioning is a best-effort step in `admin_tenants_create` + a boot backfill (mirroring the subdomain seed). The invite create (P2c) provisions the Keycloak user + fires `execute-actions-email`. Membership is created on login from the group claim (P2d-A callback — unchanged). No client secret (public client + PKCE). SMTP is written into each realm's `smtpServer` from `QUARK_KEYCLOAK_SMTP_*` so Keycloak sends the emails.

**Tech Stack:** Rust (axum, reqwest, tokio, serde). `src/codec.rs`/`src/permute.rs` UNTOUCHED. Reuses P2d-A (`oidc_configs`, login/callback, `claim_role`) + P2c (`invites`).

## Global Constraints
- English; avoid-ai-writing. **Cloud-only + opt-in**: everything gated on `st.keycloak.is_some()` (from `QUARK_KEYCLOAK_BASE_URL`). Unset → P2d-A/P2c-A behavior byte-for-byte (invite accept still creates membership; no realm calls). OSS untouched. Tests assert this parity.
- quark NEVER handles a password (Keycloak's email flow does it). No client secret stored (public client + PKCE).
- Provisioning is best-effort (never fails tenant/invite creation) + idempotent (409 realm-exists = ok) + boot backfill; mirror the subdomain-seed precedent (`admin_tenants_create` + `main.rs` backfill).
- Keycloak Admin API access via a service-account (client_credentials, `create-realm`/`manage-realm`); admin token short-lived → refetch per call / on 401.
- Unit tests use a MOCK `KeycloakAdmin` (no live Keycloak); a real-Keycloak e2e is deferred to LUC-49. PG-gated tests as usual; `-j1`.
- The derived issuer `<QUARK_KEYCLOAK_BASE_URL>/realms/<slug>` MUST match the `iss` Keycloak emits (`KC_HOSTNAME`) — a test asserts the derived issuer vs discovery when a real server is present (else documented).

## Seams (from research)
- `src/sheets/client.rs` — the mockable-trait + reqwest idiom to copy for `KeycloakAdmin`. Token via form POST (`src/oidc.rs::exchange_code`, `src/sheets/mod.rs::refresh_access_token`).
- `e2e/keycloak/quark-realm.json` — the realm/client/groups/mapper template shape (client `quark`, groups `quark-admins`/`quark-readers`, `oidc-group-membership-mapper` claim `groups` `full.path=false`).
- `admin_tenants_create` (`src/api.rs` ~2114) — best-effort side-effect + `put_oidc_config`. `main.rs` boot backfill (subdomain seed loop) — precedent for realm backfill.
- P2d-A: `TenantOidcConfig`/`put_oidc_config` (`src/oidc.rs`, `src/store/postgres.rs`), `oidc_login`/`oidc_callback`/`claim_role` (unchanged). P2c: `admin_invites_create`/`invites` (`src/api.rs`).
- `AppState` build in `main.rs` (~281) — add `keycloak: Option<KeycloakRuntime>` like `st.oidc`.

## File Structure
- Create `src/keycloak/mod.rs` (+ `client.rs`): `KeycloakAdmin` trait + `HttpKeycloakAdmin` + `KeycloakRuntime` (base, reqwest client, admin-token cache, SMTP fields) + the realm template.
- Modify `src/api.rs` (AppState field, `admin_tenants_create` provisioning, `admin_invites_create` integration), `src/main.rs` (config + backfill).
- Tests: `tests/keycloak_provision_it.rs` (new; mock-based, network-free) + additions.

---

### Task 1: `KeycloakAdmin` trait + HTTP client + config + AppState

**Files:** Create `src/keycloak/mod.rs`, `src/keycloak/client.rs`; Modify `src/api.rs` (AppState), `src/main.rs` (config); test `src/keycloak/mod.rs` unit tests.

**Produces:**
- `#[async_trait] trait KeycloakAdmin`: `async fn ensure_realm(&self, slug: &str) -> Result<(), KcError>` (create realm w/ smtpServer, idempotent 409=ok); `async fn ensure_client(&self, slug, redirect_uri) -> Result<(), KcError>` (public+PKCE client `quark`); `async fn ensure_groups_and_mapper(&self, slug) -> Result<(), KcError>` (quark-admins/quark-readers + group mapper); `async fn ensure_user(&self, slug, email, group) -> Result<String, KcError>` (create/return user id, idempotent); `async fn send_set_password_email(&self, slug, user_id) -> Result<(), KcError>` (execute-actions-email UPDATE_PASSWORD).
- `HttpKeycloakAdmin { base, client, admin_client_id, admin_secret, smtp: SmtpConfig }` with an internal `admin_token()` (client_credentials against master, cached w/ expiry, refetch on 401).
- `KeycloakRuntime` in `AppState.keycloak: Option<Arc<dyn KeycloakAdmin>>` (+ the base URL for the derived issuer). Config read in `main.rs`: `QUARK_KEYCLOAK_BASE_URL`, `_ADMIN_CLIENT_ID`, `_ADMIN_CLIENT_SECRET`, `_SMTP_HOST/_PORT/_USER/_PASSWORD/_FROM/_STARTTLS`; `None` when base URL unset.

**Steps:**
- [ ] Write failing unit tests with a `MockKeycloakAdmin` (records calls) proving the trait shape + a `derive_issuer(base, slug)` helper (`{base}/realms/{slug}`, base trailing-slash-trimmed). (The HTTP impl itself isn't unit-tested without a server — keep it thin; test the trait contract + the request-building helpers that don't need network.)
- [ ] Run, confirm fail.
- [ ] Implement `KeycloakAdmin` trait + `HttpKeycloakAdmin` (reqwest, mirror `sheets/client.rs`: `.bearer_auth`, `.json`, status checks, `KcError` mapped to String/enum; own `keycloak_client()` builder w/ timeout). `admin_token()` = form POST client_credentials, cache token+expiry. The realm-create body includes the `smtpServer` block from `SmtpConfig`. `mod keycloak;` in `src/lib.rs`.
- [ ] Add `keycloak: Option<Arc<dyn KeycloakAdmin>>` (+ derived-issuer base) to `AppState`; build in `main.rs` from env (None when `QUARK_KEYCLOAK_BASE_URL` unset). Update all `AppState` literal sites (tests/bench) — scripted insert + `cargo test --no-run`.
- [ ] Run tests; build/fmt/lib. Commit `feat(keycloak): KeycloakAdmin trait + HTTP admin client + config + AppState (opt-in)`.

---

### Task 2: realm provisioning on tenant-create + boot backfill

**Files:** Modify `src/api.rs` (`admin_tenants_create`), `src/main.rs` (backfill); test.

**Steps:**
- [ ] Write failing tests (mock `KeycloakAdmin`): creating a tenant with `st.keycloak = Some(mock)` calls ensure_realm→ensure_client→ensure_groups_and_mapper→ensure_user(owner, quark-admins)→send_set_password_email in order, then `put_oidc_config` with issuer `{base}/realms/{slug}`, client_id `quark`, empty secret, admin_value `quark-admins`, readonly_value `quark-readers`; idempotent (mock returns 409-as-ok); with `st.keycloak = None` → no realm calls, no oidc_config (pure P2d-A). Backfill: a tenant lacking an oidc_config gets provisioned on the backfill pass.
- [ ] Run, confirm fail.
- [ ] Implement: in `admin_tenants_create`, after the existing steps, `if let Some(kc) = &st.keycloak` → run the provisioning sequence best-effort (log + continue on error; do NOT fail the 201), then `put_oidc_config`. The Owner's email = the creating Principal's user email (fetch via `get_user_by_id`). Boot backfill in `main.rs`: for each tenant lacking an `oidc_config` (and `st.keycloak` set), run the same provisioning (idempotent). One-line log.
- [ ] Run tests; build/fmt/lib + gated. Commit `feat(api): auto-provision Keycloak realm per tenant on create + boot backfill`.

---

### Task 3: invite integration (provision realm user + set-password email)

**Files:** Modify `src/api.rs` (`admin_invites_create`, and the accept path for model B); test.

**Steps:**
- [ ] Write failing tests (mock): with `st.keycloak = Some`, `admin_invites_create` provisions the invited user in the realm (`ensure_user(slug, email, group)` where Admin→quark-admins, Member/Viewer→quark-readers) + `send_set_password_email`; the invite row is still recorded (pending). With `st.keycloak = None` → today's P2c behavior (no realm calls; accept still creates membership). Confirm the membership is NOT created by invite-create in model B (it comes from login).
- [ ] Run, confirm fail.
- [ ] Implement: `admin_invites_create` — after storing the invite, `if let Some(kc) = &st.keycloak` → `ensure_user` + `send_set_password_email` (best-effort; the invite still succeeds if email send fails — the Owner can re-trigger). Map the invited `Role` → group (Admin→quark-admins; Member/Viewer→quark-readers). In model B, the accept endpoint's `put_membership` is bypassed (membership from login) — gate it: when `st.keycloak.is_some()`, `admin_invites_accept` should NOT be the membership path (either 410/redirect-to-login, or leave it as a harmless no-op that says "log in via your org"). Keep model-A behavior (accept creates membership) when `st.keycloak.is_none()`. Document the split.
- [ ] Run tests; build/fmt/lib + gated. Commit `feat(api): invites provision the realm user + set-password email (model B); accept is login-driven`.

---

### Task 4: OSS/parity + security sweep + issuer-match note

**Files:** tests; docs (runbook note).

**Steps:**
- [ ] Parity tests (mostly ungated): `st.keycloak = None` → tenant-create + invite-create make ZERO Keycloak calls, and behave exactly as P2d-A/P2c-A (invite accept creates membership); OSS untouched.
- [ ] Security sweep (mock): provisioning uses only the tenant's own slug (no cross-tenant realm); the derived issuer is `{base}/realms/{slug}` (a test asserts the exact string + that `claim_role` still maps quark-admins→Admin/quark-readers→Viewer, never Owner); public client (no secret persisted — assert the stored `oidc_config.client_secret` is empty). Best-effort: a mock that errors on `ensure_realm` → tenant still created (201), backfill retries.
- [ ] Runbook: document (in `docs/` or the spec) the prod prereqs — deploy Keycloak (Fly), set `KC_HOSTNAME` == `QUARK_KEYCLOAK_BASE_URL`, create the admin service-account client, set `QUARK_KEYCLOAK_*` + `QUARK_KEYCLOAK_SMTP_*` (SendGrid: `smtp.sendgrid.net:587` user `apikey`; Resend: `smtp.resend.com:465` user `resend`; password = API key). Note the real-Keycloak e2e is LUC-49.
- [ ] Full suite green; build/clippy/fmt. Commit `test(keycloak): OSS/model-A parity + provisioning security sweep + prod runbook`.

## Verification (whole-plan)
- Mock-based unit/integration: provisioning sequence + idempotency + best-effort; invite provisions realm user + email; issuer derivation; public-client no-secret; parity when Keycloak unset.
- PG-gated where the store is touched (oidc_config written, invites).
- Real-Keycloak validation deferred to LUC-49 (e2e second realm). Opus review on Tasks 2 + 3 (auth-sensitive). Then hand the user the deploy runbook.
