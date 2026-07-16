# Multi-tenancy P1b — Auth Binding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make authentication tenant-aware — `admin_guard` resolves a `Principal { tenant, user, scopes }`, credentials (`ApiToken`/`Session`) carry their tenant and user, OIDC login populates `User`/`Membership`, and handlers access data through the tenant-scoped `ScopedStore` — plus close the P1a PK carry-overs. All resolution is still the default tenant (plumbing only; creating tenants is P2).

**Architecture:** `admin_guard` changes its success return from `()` to `Principal`, preserving its exact status contract; the ~26 call sites capture the `Principal` and scope the store via `st.store.clone().for_tenant(p.tenant)`. `ApiToken`/`Session` gain tenant/user fields (persisted+read in both backends; a new `sessions.user_id` column). The OIDC callback upserts a `User` by subject and a `Membership(user, DEFAULT_TENANT, role)`. `sheets_connection`/`wellknown_documents` get tenant-correct primary keys.

**Tech Stack:** Rust 2021, axum, sqlx (Postgres), heed (LMDB), tokio.

## Global Constraints

- All code and comments in English.
- OSS behavior is unchanged: everything resolves to `DEFAULT_TENANT` (tenant 0); the existing test suite must pass identically.
- `admin_guard`'s status contract is unchanged: 401/403/404/429/503 exactly as today; only the success path returns `Principal` instead of `()`.
- Authorization still comes from the credential's scopes (`session.scopes`/`token.scopes`/`[Full]` for the env token) — NOT recomputed from `role`. `role` on the membership is a parallel record.
- Postgres-backed tests are gated behind `QUARK_TEST_DATABASE_URL` (skip when unset).
- Postgres indexes stay plain `CREATE INDEX` — NO `CONCURRENTLY` (it deadlocks under the boot advisory lock; proven in P1a).
- PK-rework migrations are validated by a dry-run over a dump of the real prod schema+data.
- Rust tests run with `CARGO_BUILD_JOBS=1` and `cargo test -j1`.
- Out of scope: `QUARK_MULTI_TENANT` flag / any cloud-mode branch; `FORCE RLS` + `begin_tenant_tx` (P2); signup/invites/switcher/per-tenant OIDC (P2); per-tenant webhook relay (P2); `Host→tenant` (P3).

## File Structure

- **Modify** `src/tenant.rs` — add `Role::Viewer` + its `role_scopes` arm.
- **Modify** `src/auth.rs` — `ApiToken.tenant_id`, `Session.tenant_id` + `Session.user_id`, with `#[serde(default)]`.
- **Modify** `src/store/postgres.rs` — `sessions.user_id` column (migration); `get_api_token_by_hash`/`get_session_by_hash` select+populate the new fields; `put_session` writes `user_id`; the two PK reworks.
- **Modify** `src/store/lmdb.rs` — persist/read the new struct fields (JSON already round-trips whole structs; verify defaults for pre-P1b values).
- **Modify** `src/api.rs` — `Principal` struct; `admin_guard` returns `Result<Principal, StatusCode>`; ~26 call sites capture it and use `for_tenant(p.tenant)`.
- **Modify** `src/oidc.rs` — callback upserts `User`+`Membership`, builds `Session` with `tenant_id`+`user_id`.
- **Test** `tests/tenant_isolation.rs` (extend) + inline tests in the modified modules.

---

### Task 1: Add `Role::Viewer`

**Files:**
- Modify: `src/tenant.rs`
- Test: inline `#[cfg(test)]` in `src/tenant.rs`

**Interfaces:**
- Produces: `Role::Viewer`; `role_scopes(Role::Viewer) == [Scope::LinksRead, Scope::Analytics]`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn viewer_is_read_only() {
    let s = role_scopes(Role::Viewer);
    assert!(s.contains(&Scope::LinksRead));
    assert!(s.contains(&Scope::Analytics));
    assert!(!s.contains(&Scope::LinksWrite));
    assert!(!s.contains(&Scope::Full));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_BUILD_JOBS=1 cargo test -j1 --lib tenant::tests::viewer_is_read_only`
Expected: FAIL — `Role::Viewer` does not exist.

- [ ] **Step 3: Write minimal implementation**

In `src/tenant.rs`, add `Viewer` to the enum and a match arm:

```rust
pub enum Role {
    Owner,
    Admin,
    Member,
    Viewer,
}
// in role_scopes:
    Role::Viewer => &[Scope::LinksRead, Scope::Analytics],
```

- [ ] **Step 4: Run test to verify it passes**

Run: `CARGO_BUILD_JOBS=1 cargo test -j1 --lib tenant::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/tenant.rs
git commit -m "feat(tenant): add Role::Viewer (read-only) for the OIDC readonly group"
```

---

### Task 2: Credentials carry tenant + user

**Files:**
- Modify: `src/auth.rs` (`ApiToken`, `Session` structs)
- Modify: `src/store/postgres.rs` (`sessions.user_id` migration; `row_to_api_token`; `get_api_token_by_hash`, `get_session_by_hash`, `put_session` field wiring)
- Modify: `src/store/lmdb.rs` (verify whole-struct JSON round-trips the new fields)
- Test: `tests/tenant_isolation.rs` (a gated PG round-trip + an LMDB round-trip)

**Interfaces:**
- Consumes: `TenantId` (P1a), `Role::Viewer` (Task 1).
- Produces: `ApiToken.tenant_id: TenantId`; `Session.tenant_id: TenantId`, `Session.user_id: u64`. `get_api_token_by_hash` returns a token with `tenant_id` populated from the row; `get_session_by_hash` returns a session with `tenant_id`+`user_id` populated.

- [ ] **Step 1: Write the failing test** (persist a token/session under a tenant+user, read it back, assert the fields survive)

```rust
#[tokio::test]
async fn lmdb_token_and_session_carry_tenant_and_user() {
    let dir = tempfile::tempdir().unwrap();
    let store = quark::store::open_store(dir.path()).await.unwrap();
    let t = quark::tenant::TenantId(0);

    let tok = quark::auth::ApiToken {
        id: 1, name: "t".into(), token_hash: "h1".into(),
        scopes: vec![quark::auth::Scope::Full], rate_limit_per_min: None, created: 0,
        tenant_id: t,
    };
    store.put_api_token(t, &tok).await.unwrap();
    let got = store.get_api_token_by_hash("h1").await.unwrap().unwrap();
    assert_eq!(got.tenant_id, t);

    let sess = quark::auth::Session {
        token_hash: "s1".into(), subject: "sub".into(), display: "d".into(),
        scopes: vec![quark::auth::Scope::Full], created: 0, expires: u64::MAX,
        tenant_id: t, user_id: 7,
    };
    store.put_session(t, &sess).await.unwrap();
    let gs = store.get_session_by_hash("s1", 0).await.unwrap().unwrap();
    assert_eq!(gs.tenant_id, t);
    assert_eq!(gs.user_id, 7);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_BUILD_JOBS=1 cargo test -j1 --test tenant_isolation lmdb_token_and_session_carry`
Expected: FAIL — the structs have no `tenant_id`/`user_id` fields.

- [ ] **Step 3: Write minimal implementation**

`src/auth.rs` — add fields with `#[serde(default)]` so pre-P1b LMDB JSON (no field) reads as tenant 0 / user 0:

```rust
pub struct ApiToken {
    pub id: u64,
    pub name: String,
    pub token_hash: String,
    pub scopes: Vec<Scope>,
    pub rate_limit_per_min: Option<u32>,
    pub created: u64,
    #[serde(default)]
    pub tenant_id: crate::tenant::TenantId,
}

pub struct Session {
    pub token_hash: String,
    pub subject: String,
    pub display: String,
    pub scopes: Vec<Scope>,
    pub created: u64,
    pub expires: u64,
    #[serde(default)]
    pub tenant_id: crate::tenant::TenantId,
    #[serde(default)]
    pub user_id: u64,
}
```

`TenantId` needs `#[serde(default)]` support → add `#[derive(Default)]` to `TenantId` in `src/tenant.rs` (defaults to `TenantId(0)` = `DEFAULT_TENANT`):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct TenantId(pub u64);
```

`src/store/postgres.rs`:
- Add to `init_schema`'s per-table migrations: `"ALTER TABLE sessions ADD COLUMN IF NOT EXISTS user_id BIGINT NOT NULL DEFAULT 0"`.
- `row_to_api_token`: select+read `tenant_id` and set it. Update `get_api_token_by_hash`'s SELECT to include `tenant_id`:

```rust
// get_api_token_by_hash SELECT:
"SELECT id, name, token_hash, scopes, rate_limit_per_min, created, tenant_id \
 FROM api_tokens WHERE token_hash = $1"
// row_to_api_token: read tenant_id and set ApiToken.tenant_id = TenantId(r.try_get::<i64,_>("tenant_id")? as u64)
```

- `get_session_by_hash`: add `tenant_id, user_id` to the SELECT and populate the struct:

```rust
"SELECT token_hash, subject, display, scopes, created, expires, tenant_id, user_id FROM sessions \
 WHERE token_hash = $1 AND expires > $2"
// ...set tenant_id: TenantId(user_id_i64 as u64) and user_id from the row.
```

- `put_session`: add `user_id` to the INSERT column list + bind `session.user_id as i64` (write `session.tenant_id` — or keep binding the `tenant` param, which the caller sets equal to `session.tenant_id`).
- `put_api_token`: keep binding the `tenant` param (caller sets it equal to `token.tenant_id`); no column change needed (already writes tenant_id).

`src/store/lmdb.rs`: the LMDB codec serializes/deserializes whole `ApiToken`/`Session` as JSON, so the new fields round-trip automatically; `#[serde(default)]` handles pre-P1b values. No code change beyond confirming the test passes.

- [ ] **Step 4: Run test to verify it passes**

Run: `CARGO_BUILD_JOBS=1 cargo test -j1 --test tenant_isolation lmdb_token_and_session_carry`
Expected: PASS. Gated PG variant (if `QUARK_TEST_DATABASE_URL` set): mirror the test against `open_postgres` and confirm the same.

- [ ] **Step 5: Commit**

```bash
git add src/auth.rs src/tenant.rs src/store/postgres.rs src/store/lmdb.rs tests/tenant_isolation.rs
git commit -m "feat(auth): ApiToken/Session carry tenant_id + user_id (persisted both backends)"
```

---

### Task 3: `Principal` + `admin_guard` returns it + call sites adopt `ScopedStore`

**Files:**
- Modify: `src/api.rs` (`Principal` struct; `admin_guard` signature+body; ~26 call sites)
- Test: `tests/` integration (status contract) — reuse the existing admin-auth tests; add a Principal-resolution unit test.

**Interfaces:**
- Consumes: `TenantId`/`DEFAULT_TENANT` (P1a), `ApiToken.tenant_id`/`Session.tenant_id`+`user_id` (Task 2).
- Produces: `pub struct Principal { pub tenant: TenantId, pub user_id: Option<u64>, pub scopes: Vec<Scope> }`; `admin_guard(st, headers, required) -> Result<Principal, StatusCode>`.

- [ ] **Step 1: Write the failing test** (Principal resolution per credential; status contract unchanged)

```rust
// A unit test in src/api.rs #[cfg(test)] (or an integration test mirroring existing admin-auth tests):
// - env admin token present + provided  -> Ok(Principal{ tenant: DEFAULT_TENANT, user_id: None, scopes:[Full] })
// - a stored API token with scopes [LinksRead], tenant 0 -> Ok(Principal{ tenant 0, None, [LinksRead] }) for required=LinksRead
// - no credential, env token configured  -> Err(UNAUTHORIZED)   (contract preserved)
// - valid-but-insufficient token          -> Err(FORBIDDEN)      (contract preserved)
// Assert both the Ok Principal contents AND that the error statuses are unchanged.
```

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_BUILD_JOBS=1 cargo test -j1 --lib api::` (or the integration test name)
Expected: FAIL — `admin_guard` returns `()`, `Principal` undefined.

- [ ] **Step 3: Write minimal implementation**

Add the struct and change `admin_guard` (`src/api.rs:1273`). Preserve the status logic verbatim; each success `return Ok(())` becomes `return Ok(Principal { ... })`:

```rust
pub struct Principal {
    pub tenant: crate::tenant::TenantId,
    pub user_id: Option<u64>,
    pub scopes: Vec<Scope>,
}

async fn admin_guard(st: &AppState, headers: &HeaderMap, required: Scope)
    -> Result<Principal, StatusCode>
{
    // ...unchanged provided/not_found_status/flags logic...

    // 1) env admin token:
    if let Some(expected) = st.admin_token.as_deref() {
        if constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
            return Ok(Principal { tenant: crate::tenant::DEFAULT_TENANT, user_id: None, scopes: vec![Scope::Full] });
        }
    }
    // 2) API token — on the covering / rate-ok branches:
    //    return Ok(Principal { tenant: token.tenant_id, user_id: None, scopes: token.scopes.clone() });
    // 3) OIDC session — on the covering branch:
    //    return Ok(Principal { tenant: session.tenant_id, user_id: Some(session.user_id), scopes: session.scopes.clone() });
    // ...all Err(...) paths unchanged (503/429/403/not_found_status).
}
```

Update the ~26 call sites (grep `admin_guard(` in `src/api.rs`). Two mechanical patterns:

```rust
// pattern A (was: `if let Err(status) = admin_guard(...).await { return status.into_response(); }`)
let p = match admin_guard(&st, &headers, Scope::X).await {
    Ok(p) => p,
    Err(status) => return status.into_response(),
};
let store = st.store.clone().for_tenant(p.tenant);
// ...use `store` for tenant-owned access instead of the raw `st.store.<m>(DEFAULT_TENANT, ...)`.

// pattern B (line 331, `create`: `admin_guard(...).await` returning the status directly)
// becomes `admin_guard(...).await.map(|_| ())` where only the gate matters, or capture `p`
// where the handler needs the tenant.
```

For handlers that only gate (don't touch tenant-owned data), capturing `p` and ignoring it is fine; for the rest, replace `st.store.clone().for_tenant(crate::tenant::DEFAULT_TENANT)` (the P1a raw default) with `st.store.clone().for_tenant(p.tenant)`.

- [ ] **Step 4: Run test to verify it passes**

Run: `CARGO_BUILD_JOBS=1 cargo test -j1` (full suite — the existing admin-auth/status tests are the contract guard)
Expected: PASS, including the pre-existing status-contract tests unchanged.

- [ ] **Step 5: Commit**

```bash
git add src/api.rs
git commit -m "feat(api): admin_guard returns Principal (tenant+user+scopes); handlers scope via for_tenant"
```

---

### Task 4: OIDC login populates `User`/`Membership`

**Files:**
- Modify: `src/oidc.rs` (callback: upsert user+membership, build session with tenant+user)
- Test: `tests/` (an OIDC callback path test, or a unit test of the upsert helper)

**Interfaces:**
- Consumes: `Store` identity methods from P1a (`get_user_by_subject`, `put_user`, `next_user_id`, `put_membership`); `Session.tenant_id`+`user_id` (Task 2); `Role::Admin`/`Role::Viewer` (Task 1).
- Produces: after a successful callback, a `User` row keyed by `subject` and a `Membership(user, DEFAULT_TENANT, role)` exist; the created `Session` carries `tenant_id = DEFAULT_TENANT` and `user_id`.

- [ ] **Step 1: Write the failing test**

```rust
// Drive the post-token-validation path (factor an upsert helper if the callback is hard to
// test end-to-end): given a subject "sub-1" and the admin group, assert:
// - get_user_by_subject("sub-1") is Some after; its id is stable on a 2nd call (no dup).
// - get_membership(user.id, DEFAULT_TENANT) is Some with role Admin.
// - the built Session has tenant_id == DEFAULT_TENANT and user_id == user.id.
// And with the readonly group -> membership role == Viewer.
```

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_BUILD_JOBS=1 cargo test -j1 <oidc test name>`
Expected: FAIL — no user/membership created; session lacks fields.

- [ ] **Step 3: Write minimal implementation**

In the OIDC callback (`src/oidc.rs`), after `map_scopes` yields the scopes (and thus which group matched), before creating the session:

```rust
// Resolve/create the user by the immutable subject.
let user = match st.store.get_user_by_subject(&subject).await? {
    Some(u) => u,
    None => {
        let id = st.store.next_user_id().await?;
        let u = crate::tenant::User { id, subject: subject.clone(), email: email.clone(), display: display.clone(), created: now() };
        st.store.put_user(&u).await?;
        u
    }
};
// Role aligned with the same group that produced the scopes.
let role = if scopes.iter().any(|s| *s == Scope::Full) { crate::tenant::Role::Admin }
           else { crate::tenant::Role::Viewer };
st.store.put_membership(&crate::tenant::Membership {
    user_id: user.id, tenant_id: crate::tenant::DEFAULT_TENANT, role, created: now(),
}).await?;
// Session carries the tenant + user.
let session = crate::auth::Session {
    token_hash, subject, display, scopes,
    created: now(), expires: now() + SESSION_TTL_SECS,
    tenant_id: crate::tenant::DEFAULT_TENANT, user_id: user.id,
};
st.store.put_session(crate::tenant::DEFAULT_TENANT, &session).await?;
```

(Adapt variable names to the actual callback; authorization still flows from `scopes`, unchanged.)

- [ ] **Step 4: Run test to verify it passes**

Run: `CARGO_BUILD_JOBS=1 cargo test -j1 <oidc test name>`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/oidc.rs
git commit -m "feat(oidc): login upserts User+Membership (tenant 0) and binds Session to the user"
```

---

### Task 5: PK reworks — `sheets_connection` + `wellknown_documents`

**Files:**
- Modify: `src/store/postgres.rs` (`init_schema` migration; `put_sheets_connection`; `put_wellknown`)
- Test: `tests/tenant_isolation.rs` (gated PG round-trip under the new PKs + idempotency)

**Interfaces:**
- Produces: `sheets_connection` keyed by `(tenant_id)` (no `singleton`); `wellknown_documents` keyed by `(tenant_id, name)`.

- [ ] **Step 1: Write the failing test**

```rust
// Gated PG (returns early if QUARK_TEST_DATABASE_URL unset):
// - two tenants each put a wellknown doc with the SAME name -> both coexist, each reads back its own.
// - a tenant put_sheets_connection twice -> upsert (one row per tenant), reads back the latest.
```

- [ ] **Step 2: Run test to verify it fails**

Run: `QUARK_TEST_DATABASE_URL=... CARGO_BUILD_JOBS=1 cargo test -j1 --test tenant_isolation <pk test name>`
Expected: FAIL — `wellknown_documents` PK on `name` alone rejects the 2nd tenant's same-name doc / sheets singleton collides.

- [ ] **Step 3: Write minimal implementation**

`src/store/postgres.rs` `init_schema` — idempotent PK migration (guarded so re-runs are no-ops). Because `CREATE TABLE IF NOT EXISTS` won't alter an existing table, use explicit statements:

```sql
-- sheets_connection: drop the singleton PK, key on tenant_id
ALTER TABLE sheets_connection DROP CONSTRAINT IF EXISTS sheets_connection_pkey;
ALTER TABLE sheets_connection DROP COLUMN IF EXISTS singleton;
-- add PK only if not present (guard via a DO block or the catalog):
DO $$ BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname='sheets_connection_pkey') THEN
    ALTER TABLE sheets_connection ADD PRIMARY KEY (tenant_id);
  END IF;
END $$;

-- wellknown_documents: PK (tenant_id, name)
ALTER TABLE wellknown_documents DROP CONSTRAINT IF EXISTS wellknown_documents_pkey;
DO $$ BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname='wellknown_documents_pkey') THEN
    ALTER TABLE wellknown_documents ADD PRIMARY KEY (tenant_id, name);
  END IF;
END $$;
```

(Run each via `sqlx::query(...)`; the `DO $$ ... $$` block executes fine on the extended protocol. Keep the fresh-DB `CREATE TABLE` definitions updated too: `sheets_connection (tenant_id BIGINT NOT NULL DEFAULT 0 PRIMARY KEY, blob JSONB NOT NULL)` and drop the `sheets_connection_by_tenant` unique index created in P1a since the PK now covers it — or leave the index, harmless. Prefer dropping it to avoid redundancy.)

`put_sheets_connection`: remove `singleton` from the INSERT; `ON CONFLICT (tenant_id) DO UPDATE`.
`put_wellknown`: `ON CONFLICT (tenant_id, name) DO UPDATE SET body = EXCLUDED.body`.

- [ ] **Step 4: Run test to verify it passes**

Run: `QUARK_TEST_DATABASE_URL=... CARGO_BUILD_JOBS=1 cargo test -j1 --test tenant_isolation <pk test name>`
Expected: PASS. Re-run twice to confirm migration idempotency.

- [ ] **Step 5: Commit**

```bash
git add src/store/postgres.rs tests/tenant_isolation.rs
git commit -m "feat(store/pg): tenant-correct PKs for sheets_connection and wellknown_documents"
```

---

### Task 6: OSS parity + Principal/login broad tests

**Files:**
- Modify: `tests/tenant_isolation.rs` (or a new `tests/auth_binding.rs`)
- Test: the whole suite for OSS parity.

**Interfaces:**
- Consumes: everything above.

- [ ] **Step 1: Add the regression assertions**

```rust
// - OSS parity: full pre-existing suite passes unchanged (run below).
// - A default-tenant end-to-end: create a link via the create path (admin-gated), confirm it is
//   readable, confirming the admin_guard->Principal->for_tenant(0) chain is behavior-preserving.
```

- [ ] **Step 2: Run OSS parity**

Run: `CARGO_BUILD_JOBS=1 cargo test -j1`
Expected: the pre-existing lib + integration suites PASS unchanged (Postgres-gated tests only run with `QUARK_TEST_DATABASE_URL`).

- [ ] **Step 3: Commit**

```bash
git add tests/
git commit -m "test(auth): Principal resolution + OSS parity for tenant-bound auth"
```

---

## Self-Review

**1. Spec coverage** (against `2026-07-16-multi-tenancy-p1b-auth-binding-design.md`):
- Role::Viewer → Task 1. ✓
- ApiToken/Session carry tenant+user, both backends, serde defaults, sessions.user_id column → Task 2. ✓
- Principal + admin_guard return + status contract + ScopedStore adoption at call sites → Task 3. ✓
- OIDC login upserts User/Membership + role mapping (admin→Admin, readonly→Viewer) + Session binding → Task 4. ✓
- PK reworks sheets_connection + wellknown_documents, idempotent migration → Task 5. ✓
- OSS parity + Principal tests → Task 6. ✓
- Out-of-scope items (flag, FORCE RLS, signup, per-tenant relay, Host→tenant) → absent. ✓
- Indexes stay non-CONCURRENTLY → no new CONCURRENTLY introduced. ✓

**2. Placeholder scan:** Test bodies in Tasks 3/4/6 describe assertions in prose comments rather than full compiled code because they hook into existing test harnesses (admin-auth integration tests, the OIDC callback) whose exact fixtures live in the repo; the implementer wires them to the real harness. Every production-code step has concrete code. Flagged for the implementer.

**3. Type consistency:** `Principal { tenant, user_id, scopes }`, `ApiToken.tenant_id`, `Session.tenant_id`+`user_id`, `Role::Viewer`, `TenantId: Default` used consistently across tasks. `admin_guard` return type `Result<Principal, StatusCode>` matches all call-site updates in Task 3.
