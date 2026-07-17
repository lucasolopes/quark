# Multi-tenancy P2d-backend (OIDC per-tenant) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Per-tenant OIDC (cloud-only): a tenant configures its own IdP; login resolves which IdP via a slug (`/login?org=acme`); the callback validates against that tenant's config and creates a membership with the role from the IdP's group claim. The global env OIDC stays as platform/OSS login. Break-glass admin token untouched.

**Architecture:** New `oidc_configs` table (TENANT_OWNED + NOT_FORCED — read by slug→tenant at login before RLS context exists). `OidcRuntime` becomes buildable from a stored config + cached per tenant. The signed `qk_login` cookie carries the tenant id so the callback knows which config to validate against. Cloud login maps the IdP admin-group claim to the membership `Role`.

**Tech Stack:** Rust (axum, sqlx, jsonwebtoken, the existing `src/oidc.rs`). `src/codec.rs`/`src/permute.rs` UNTOUCHED.

## Global Constraints
- English; avoid-ai-writing. Cloud-only; the global env OIDC (`st.oidc`) + OSS + break-glass admin token behave byte-for-byte as today (tests assert it). `admin_guard` status contract (401/403/404/429/503) preserved.
- `client_secret` stored plaintext in the `blob` JSONB (Sheets precedent; hardening is a separate issue). `oidc_configs` RLS ENABLE + NOT_FORCED; admin CRUD tenant-scoped, login/callback read bare.
- A `?org` with no config must NOT fall back to the global OIDC (identity confusion) — explicit error.
- No `CREATE INDEX CONCURRENTLY`. PG gated, NON-SUPERUSER cloud verification (final gate). `-j1`.
- Auth is historically bug-prone here — the login/callback task gets an Opus review.

## Seams (from the P2d map)
- `src/oidc.rs`: `OidcConfig`/`from_env` (`:20-60`), `OidcRuntime` (`:380`, discovery+`RwLock<Jwks>`), `sign_login_state`/`verify_login_state` (`:457-478`), `ensure_user_and_membership` (`:335`), `map_scopes` (`:303`), `verify_id_token` (`:238`).
- `src/api.rs`: `oidc_login` (`:1547`), `oidc_callback` (`:1590`), `AppState.oidc`/`oidc_configured` (`:48/:52`), routes `/admin/login` `/admin/callback` (`:3878-3879`), `admin_guard` (`:1384`), `admin_me` (`:1724`).
- `src/store/*`: `tenants.slug` UNIQUE; `get_tenant`; TENANT_OWNED_TABLES + NOT_FORCED + `reset_for_tests`.
- `src/main.rs`: builds `st.oidc` from env (`:231-251`).
- Frontend (P2d-frontend, separate): `web/src/routes/Login.tsx`, `oidcLoginUrl` (`web/src/lib/api.ts:49`).

---

### Task 1: `oidc_configs` table + type + store + `get_tenant_by_slug`

**Files:** `src/oidc.rs` (or `src/tenant_oidc.rs`) type; `src/store/mod.rs`/`postgres.rs`/`lmdb.rs`; test `tests/oidc_config_it.rs` (new) or extend `workspace_it`.

**Produces:**
- `TenantOidcConfig { tenant_id: TenantId, issuer, client_id, client_secret, scopes: Vec<String>, admin_claim, admin_value, readonly_value, post_login_url: Option<String> }` (serde; the non-tenant fields also serialize to/from the `blob`).
- Store: `next_oidc_config_id`; `put_oidc_config(&TenantOidcConfig)` (upsert on UNIQUE tenant_id, tenant-scoped write); `get_oidc_config(tenant)` (tenant-scoped read, for admin CRUD); `get_oidc_config_bare(tenant)` (bare pool, for login/callback); `delete_oidc_config(tenant)`; `get_tenant_by_slug(slug) -> Option<Tenant>` (bare pool).

**Steps:**
- [ ] Failing PG-gated test: put a config for tenant A; `get_oidc_config(A)` returns it (secret round-trips via blob); `get_oidc_config(B)` (tenant-scoped) does not see A's; `get_oidc_config_bare(A)` returns it; `get_tenant_by_slug("acme")` → tenant A; unknown slug → None; upsert replaces (still one row).
- [ ] Run, confirm fail.
- [ ] Type in `src/oidc.rs` (re-export). `init_schema`: `CREATE SEQUENCE IF NOT EXISTS quark_oidc_config_id_seq START WITH 1`; `CREATE TABLE IF NOT EXISTS oidc_configs (id BIGINT PRIMARY KEY, tenant_id BIGINT NOT NULL DEFAULT 0, issuer TEXT NOT NULL, blob JSONB NOT NULL, created BIGINT NOT NULL)`; `CREATE UNIQUE INDEX IF NOT EXISTS oidc_configs_tenant_idx ON oidc_configs (tenant_id)`; add `"oidc_configs"` to `TENANT_OWNED_TABLES` + `NOT_FORCED` (bump lengths; comment: read by slug→tenant at login before RLS ctx). Add to `reset_for_tests`. NO CONCURRENTLY.
- [ ] Impl the store methods: put/get/delete tenant-scoped via `with_write!`/`with_read!`; `get_oidc_config_bare` + `get_tenant_by_slug` on the bare pool. Serialize the non-tenant fields into `blob` (serde_json), mirroring `sheets_connection`. OSS backend: stubs (`Unsupported`/None).
- [ ] Run test → pass; build/fmt/lib. Commit `feat(store): oidc_configs table + TenantOidcConfig + get_tenant_by_slug`.

---

### Task 2: admin CRUD `/admin/oidc-config` (Owner/Admin, cloud-only)

**Files:** `src/api.rs` (handlers + routes); test append.

**Produces:** `PUT /admin/oidc-config {issuer, client_id, client_secret, scopes, admin_claim, admin_value, readonly_value, post_login_url?}` → 200; `GET /admin/oidc-config` → 200 (config WITHOUT client_secret, e.g. `client_secret_set: bool`); `DELETE /admin/oidc-config` → 200/404.

**Steps:**
- [ ] Failing tests: PUT as Owner/Admin upserts the tenant's config (stored secret round-trips); GET returns the config with the secret redacted (never returns `client_secret`); DELETE removes it; a Member/Viewer (non-Full) → 403; all → 404 when `!multi_tenant`.
- [ ] Run, confirm fail.
- [ ] Implement: each handler `if !st.multi_tenant {404}`; `admin_guard(Scope::Full)` (Owner/Admin); rate-limit the PUT. PUT builds `TenantOidcConfig{tenant_id: p.tenant, ..}` + `put_oidc_config`; invalidate the per-tenant runtime cache (Task 3 hook — for now a no-op or the cache's `invalidate`). GET → a redacted view struct (`client_secret_set: !empty`, never the value). Register routes near `/admin/tenants`.
- [ ] Run → pass; build/fmt/lib. Commit `feat(api): /admin/oidc-config CRUD (Owner/Admin, cloud-only, secret redacted on read)`.

---

### Task 3: per-tenant `OidcRuntime` cache + tenant in the login-state cookie

**Files:** `src/oidc.rs` (`OidcRuntime::from_tenant_config`, cache, extend sign/verify_login_state); `src/api.rs`/`src/main.rs` (AppState cache field); test.

**Produces:**
- `OidcRuntime::from_config(cfg: &TenantOidcConfig) -> Result<OidcRuntime>` (mirrors `init` but from a stored config; discovery + JWKS fetch). A cache on `AppState` (e.g. `oidc_tenants: moka::future::Cache<u64, Arc<OidcRuntime>>` keyed by tenant_id) with `get_or_build(tenant, cfg)` + `invalidate(tenant)`; TTL so a reconfig isn't stuck.
- `sign_login_state`/`verify_login_state` extended to carry an optional `tenant_id` (a 4th field) in the HMAC-signed payload; back-compat: absence = global login.

**Steps:**
- [ ] Failing unit tests (in `src/oidc.rs`): `sign_login_state` with a tenant round-trips through `verify_login_state` (tenant recovered); tampering with the tenant field fails the MAC; absent tenant → None (global). Cache: `get_or_build` builds once (discovery called once via a fake), `invalidate` drops it. (Use a fake discovery/JWKS seam or a constructor that doesn't hit network for the cache test; if discovery requires network, unit-test only the sign/verify + cache-keying logic and defer the build to the integration path.)
- [ ] Run, confirm fail.
- [ ] Implement the cookie extension (keep the existing 3-field format working — append the tenant as an optional 4th segment; verify recomputes the MAC over all present fields). Implement `from_config` + the cache + AppState field (thread through main.rs/tests — scripted AppState-literal update, `cargo test --no-run`). The Task 2 PUT/DELETE call `invalidate(tenant)`.
- [ ] Run → pass; build/fmt/lib. Commit `feat(oidc): per-tenant OidcRuntime cache + tenant carried in signed login-state`.

---

### Task 4 (SECURITY-CRITICAL, Opus review): login `?org=` + callback per-tenant

**Files:** `src/api.rs` (`oidc_login`, `oidc_callback`); `src/oidc.rs` (`ensure_user_and_membership` cloud-with-tenant path + claim→role); test.

**Steps:**
- [ ] Failing tests (http-level + oidc unit): `oidc_login?org=acme` → resolves tenant by slug, loads its config, builds the authorize URL against the tenant's issuer, and the `qk_login` cookie carries tenant A; `?org=` unknown/no-config → explicit error (NOT the global flow); no `?org` → global env flow unchanged. `oidc_callback` with a `qk_login` carrying tenant A → validates the id-token against A's config (issuer/aud/JWKS), and `ensure_user_and_membership` creates/updates `Membership(user, A, role)` where role = claim mapping (admin_value→Admin, readonly_value→Viewer, else Member); session tenant = A. A token from the wrong issuer → rejected. Global callback (no tenant in cookie) → unchanged.
- [ ] Run, confirm fail.
- [ ] Implement `oidc_login`: parse `Query<{org: Option<String>}>`. `Some(slug)` → `get_tenant_by_slug` → `get_oidc_config_bare` → `oidc_tenants.get_or_build` → authorize URL; sign tenant into `qk_login`. Missing tenant/config → 4xx with a clear message. `None` → today's global path.
- [ ] Implement `oidc_callback`: recover the tenant from `verify_login_state`. `Some(tenant)` → use that tenant's runtime for token exchange + `verify`; call a cloud-with-tenant `ensure_user_and_membership` that upserts the User AND a `Membership(user, tenant, role_from_claim)`; session `tenant_id = tenant`. `None` → today's global path (cloud still creates no membership, as P2b). Map the claim to `Role` (new helper `claim_role(claims, cfg) -> Role`). Preserve status contract; break-glass/API-token untouched.
- [ ] Run → pass; build/fmt/lib. Commit `feat(api): OIDC login by org slug + per-tenant callback validation + membership from claim role`.

---

### Task 5: OSS parity + security sweep (tests-heavy)

**Files:** tests; small fixes only if a gap is found.

**Steps:**
- [ ] Ungated OSS test: `PUT/GET/DELETE /admin/oidc-config` → 404 in OSS. Global login/callback (no `?org`) identical to pre-P2d.
- [ ] Security sweep (PG-gated non-superuser): tenant A's oidc-config not visible/editable by tenant B; GET never returns the client_secret; a tampered `qk_login` tenant field fails; a `?org` without config does not silently use the global IdP; break-glass admin token still Scope::Full tenant 0 with OIDC-per-tenant configured; login into A yields a session/membership only in A (no cross-tenant scope).
- [ ] Full suite green; build/clippy/fmt. Commit `test(oidc): OSS parity + per-tenant security sweep`.

## Verification (whole-plan)
- PG-gated NON-SUPERUSER cloud: config CRUD tenant-scoped + secret redaction; login-by-slug + callback membership-from-claim; wrong-issuer rejection; tampered-cookie rejection; no global fallback on missing config.
- OSS/global parity: env OIDC + break-glass byte-for-byte; existing oidc tests pass.
- Opus review on Task 4. Then P2d-frontend (org input on Login) as a follow-on plan.
