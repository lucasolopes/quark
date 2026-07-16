# Multi-tenancy P2b-backend — Signup & Workspace Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** In cloud mode, let any authenticated OIDC user create their own workspace (becoming Owner), carry the current workspace in the session, switch workspaces (membership-checked), and authorize by the user's role in the current tenant — while OSS mode stays byte-for-byte the same.

**Architecture:** A `next_tenant_id` allocator + `POST /admin/tenants` create a Tenant + Owner Membership (cloud only). `session.tenant_id` is the current workspace; `POST /admin/workspace/switch` changes it after verifying membership. `admin_guard`'s OIDC-session branch, in cloud, resolves the user's membership at `session.tenant_id` and derives scopes from the role; OSS keeps the group→scope map. Cloud login upserts the User but creates NO tenant-0 membership; `/admin/me` returns memberships + current tenant so the frontend (P2b-frontend) can route to onboarding vs switcher.

**Tech Stack:** Rust 2021, axum, sqlx (Postgres), tokio.

## Global Constraints

- All code/comments in English.
- OSS mode (`QUARK_MULTI_TENANT` off) unchanged: login → tenant 0 membership, group→scope, existing suite passes identically.
- `admin_guard` status contract unchanged: 401/403/404/429/503 exactly; env-admin-token and API-token paths unchanged; only the OIDC-session cloud branch changes its scope source.
- SECURITY invariants: `workspace/switch` ALWAYS verifies the user has a membership in the target tenant before changing the session; cloud login creates NO automatic tenant-0 membership; a session whose user has no membership in the current tenant → 403.
- No `CREATE INDEX CONCURRENTLY`. `src/codec.rs`/`src/permute.rs` untouched.
- Postgres tests gated behind `QUARK_TEST_DATABASE_URL`. `CARGO_BUILD_JOBS=1 cargo test -j1`.
- **Controller verifies before merge:** gated arm against a real NON-SUPERUSER Postgres in cloud mode + a signup→switch→role-authorization end-to-end.
- Out of scope: UI (P2b-frontend); invites (P2c); per-tenant OIDC (P2d).

## File Structure

- **Modify** `src/store/mod.rs` — `next_tenant_id()` on the `Store` trait.
- **Modify** `src/store/postgres.rs` — `quark_tenant_id_seq` (start 1) + `next_tenant_id`; global/infra method (no tenant-tx).
- **Modify** `src/store/lmdb.rs` — `next_tenant_id` via a `meta` counter (unused in OSS single-tenant, present for the trait).
- **Modify** `src/webhooks/delivery.rs` — the test-stub `Store` impl gains `next_tenant_id` (`unimplemented!()`), to keep it compiling.
- **Modify** `src/oidc.rs` — `ensure_user_and_membership` becomes mode-aware (cloud: user only, no membership).
- **Modify** `src/api.rs` — `POST /admin/tenants`, `POST /admin/workspace/switch`, `admin_guard` cloud role resolution, `admin_me` memberships+current, route registration.
- **Test** `tests/workspace_it.rs` (new) + extend `tests/tenant_enforcement.rs`.

---

### Task 1: `next_tenant_id` allocator

**Files:**
- Modify: `src/store/mod.rs` (trait), `src/store/postgres.rs`, `src/store/lmdb.rs`, `src/webhooks/delivery.rs` (test stub)
- Test: inline in `src/store/postgres.rs` / gated round-trip in a test

**Interfaces:**
- Produces: `async fn next_tenant_id(&self) -> Result<u64, StoreError>` on `Store`. Postgres: `nextval('quark_tenant_id_seq')` (sequence created in `init_schema`, starts at 1 so it never collides with the seeded default tenant 0). LMDB: `meta["next_tenant_id"]` counter. Global/infra — NOT routed through a tenant-tx.

- [ ] **Step 1: Write the failing test**

```rust
// gated PG (tests/workspace_it.rs) — ids are >=1 and monotonic, never 0
#[tokio::test]
async fn next_tenant_id_starts_above_default() {
    let Some(url) = std::env::var("QUARK_TEST_DATABASE_URL").ok() else { return; };
    let store = quark::store::PostgresStore::open(&url, true).await.unwrap();
    let a = store.next_tenant_id().await.unwrap();
    let b = store.next_tenant_id().await.unwrap();
    assert!(a >= 1 && b > a, "tenant ids must be >=1 (0 is the default tenant) and monotonic");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `QUARK_TEST_DATABASE_URL=... CARGO_BUILD_JOBS=1 cargo test -j1 --test workspace_it next_tenant_id`
Expected: FAIL — method missing.

- [ ] **Step 3: Write minimal implementation**

`src/store/mod.rs` trait: `async fn next_tenant_id(&self) -> Result<u64, StoreError>;`

`src/store/postgres.rs`: in `init_schema` add `"CREATE SEQUENCE IF NOT EXISTS quark_tenant_id_seq START WITH 1"`; impl:
```rust
async fn next_tenant_id(&self) -> Result<u64, StoreError> {
    let id: i64 = sqlx::query_scalar("SELECT nextval('quark_tenant_id_seq')")
        .fetch_one(&self.write).await.map_err(StoreError::backend)?;
    Ok(id as u64)
}
```
(Bare pool — global/infra, no tenant-tx.)

`src/store/lmdb.rs`: mirror the other `next_*` counters in `meta` (`next_tenant_id`, start at 1).

`src/webhooks/delivery.rs` test-stub impl: `async fn next_tenant_id(&self) -> Result<u64, StoreError> { unimplemented!() }`.

- [ ] **Step 4: Run test to verify it passes**

Run: `QUARK_TEST_DATABASE_URL=... CARGO_BUILD_JOBS=1 cargo test -j1 --test workspace_it next_tenant_id` + `CARGO_BUILD_JOBS=1 cargo build -j1`
Expected: PASS + compiles.

- [ ] **Step 5: Commit**

```bash
git add -u && git commit -m "feat(store): next_tenant_id allocator (seq starts at 1; 0 is default tenant)"
```

---

### Task 2: Cloud login creates no auto tenant-0 membership

**Files:**
- Modify: `src/oidc.rs` (`ensure_user_and_membership`), and its caller in `src/api.rs` (the OIDC callback)
- Test: `src/oidc.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: nothing new.
- Produces: `ensure_user_and_membership(store, multi_tenant: bool, subject, email, display, scopes) -> Result<u64, StoreError>` — cloud (`multi_tenant=true`): upsert the `User` by subject, return its id, create NO membership. OSS (`false`): current behavior (upsert User + `Membership(user, DEFAULT_TENANT, role)`).

- [ ] **Step 1: Write the failing test**

```rust
// in src/oidc.rs #[cfg(test)]
#[tokio::test]
async fn cloud_login_creates_user_but_no_default_membership() {
    let dir = tempfile::tempdir().unwrap();
    let store = crate::store::lmdb::LmdbStore::open_with_node_id(dir.path(), None).unwrap();
    let uid = ensure_user_and_membership(&store, /*multi_tenant=*/true, "sub-cloud", "e@x", "E", &[Scope::Full]).await.unwrap();
    assert!(store.get_user_by_subject("sub-cloud").await.unwrap().is_some());
    // no membership was created in the default tenant
    assert!(store.list_memberships_for_user(uid).await.unwrap().is_empty());
}
// keep the existing OSS test (multi_tenant=false) asserting the tenant-0 membership IS created.
```

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_BUILD_JOBS=1 cargo test -j1 --lib oidc::tests::cloud_login_creates_user_but_no_default_membership`
Expected: FAIL — signature has no `multi_tenant`, and today it always creates the membership.

- [ ] **Step 3: Write minimal implementation**

In `src/oidc.rs` `ensure_user_and_membership` add the `multi_tenant: bool` param; after the user upsert:
```rust
if !multi_tenant {
    // OSS: single implicit tenant 0. Cloud: no membership until the user
    // creates or is invited to a workspace (P2b/P2c).
    let role = if scopes.iter().any(|s| *s == Scope::Full) { Role::Admin } else { Role::Viewer };
    store.put_membership(&Membership { user_id: user.id, tenant_id: DEFAULT_TENANT, role, created: now() }).await?;
}
Ok(user.id)
```
Update the caller in `src/api.rs` (OIDC callback) to pass `st.multi_tenant`.

- [ ] **Step 4: Run test to verify it passes**

Run: `CARGO_BUILD_JOBS=1 cargo test -j1 --lib oidc::tests`
Expected: PASS (both the cloud and the OSS membership tests).

- [ ] **Step 5: Commit**

```bash
git add -u && git commit -m "feat(oidc): cloud login upserts user without auto tenant-0 membership"
```

---

### Task 3: `POST /admin/tenants` — create workspace

**Files:**
- Modify: `src/api.rs` (handler + route), `src/webhooks`/store as needed
- Test: `tests/workspace_it.rs` (gated) + a non-gated route/OSS-disabled test

**Interfaces:**
- Consumes: `next_tenant_id` (Task 1), `Principal.user_id`, `put_tenant`, `put_membership`, `Role::Owner`.
- Produces: `POST /admin/tenants` body `{ "name": String, "slug": String }` → creates `Tenant` + `Membership(user, new_tenant, Owner)`, sets the session's current tenant to the new one, returns the tenant. Cloud only (OSS → 404). Rate-limited (reuse `st.ratelimiter`, keyed on the user/ip).

- [ ] **Step 1: Write the failing test**

```rust
// tests/workspace_it.rs (gated, cloud). Using the OIDC-session harness (a session for a user).
// - POST /admin/tenants {name,slug} -> 200, returns tenant with id>=1
// - get_membership(user, new_tenant) is Owner
// - the session's tenant_id is now the new tenant
// - a 2nd create with the SAME slug -> 409 (unique slug)
// - in OSS mode the route is 404
```

- [ ] **Step 2: Run test to verify it fails**

Run: `QUARK_TEST_DATABASE_URL=... CARGO_BUILD_JOBS=1 cargo test -j1 --test workspace_it create_tenant`
Expected: FAIL — route not found.

- [ ] **Step 3: Write minimal implementation**

Add the handler in `src/api.rs`:
```rust
async fn admin_tenants_create(State(st): State<Arc<AppState>>, headers: HeaderMap, Json(req): Json<CreateTenantReq>) -> Response {
    if !st.multi_tenant { return StatusCode::NOT_FOUND.into_response(); }
    // Any authenticated OIDC user can create a workspace. Resolve the user from the session
    // (NOT admin_guard's role check — a user with 0 memberships must still be able to create one).
    let Some(user_id) = session_user_id(&st, &headers).await else { return StatusCode::UNAUTHORIZED.into_response(); };
    let ip = client_ip(&headers, &st.real_ip_header, /*conn*/ None);
    if !st.ratelimiter.check(&ip, now()).await { return (StatusCode::TOO_MANY_REQUESTS, "too many requests").into_response(); }
    let id = match st.store.next_tenant_id().await { Ok(i) => i, Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response() };
    let tenant = Tenant { id: TenantId(id), name: req.name, slug: req.slug, created: now() };
    // put_tenant returns a unique-violation error on duplicate slug -> map to 409.
    if let Err(e) = st.store.put_tenant(&tenant).await { return conflict_or_503(e).into_response(); }
    st.store.put_membership(&Membership { user_id, tenant_id: TenantId(id), role: Role::Owner, created: now() }).await.ok();
    // set the new tenant as the session's current workspace
    set_session_tenant(&st, &headers, TenantId(id)).await;
    Json(tenant).into_response()
}
```
Introduce helpers: `session_user_id(st, headers) -> Option<u64>` (reads the session cookie → session.user_id; cloud only); `set_session_tenant(st, headers, tenant)` (re-puts the session with the new `tenant_id`); `conflict_or_503(StoreError)` (unique-violation → 409, else 503). Register the route: `.route("/admin/tenants", post(admin_tenants_create))`. `put_tenant` must surface a distinguishable unique-violation error on the `slug`/`id` conflict (Postgres unique on `slug`).

- [ ] **Step 4: Run test to verify it passes**

Run: `QUARK_TEST_DATABASE_URL=... CARGO_BUILD_JOBS=1 cargo test -j1 --test workspace_it create_tenant`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -u && git commit -m "feat(api): POST /admin/tenants — self-serve workspace creation (cloud)"
```

---

### Task 4: `POST /admin/workspace/switch` — change current workspace

**Files:**
- Modify: `src/api.rs` (handler + route)
- Test: `tests/workspace_it.rs` (gated)

**Interfaces:**
- Consumes: `session_user_id`, `set_session_tenant` (Task 3), `get_membership`.
- Produces: `POST /admin/workspace/switch` body `{ "tenant_id": u64 }` → if the user has a membership in that tenant, updates the session's `tenant_id` and returns 200; else 403 (does NOT change the session).

- [ ] **Step 1: Write the failing test**

```rust
// tests/workspace_it.rs (gated, cloud):
// - user is Owner of tenant A (created in Task 3); switch to A -> 200, session.tenant_id == A
// - switch to tenant B where the user has NO membership -> 403, session.tenant_id UNCHANGED (still A)
```

- [ ] **Step 2: Run test to verify it fails**

Run: `QUARK_TEST_DATABASE_URL=... CARGO_BUILD_JOBS=1 cargo test -j1 --test workspace_it switch`
Expected: FAIL — route missing.

- [ ] **Step 3: Write minimal implementation**

```rust
async fn admin_workspace_switch(State(st): State<Arc<AppState>>, headers: HeaderMap, Json(req): Json<SwitchReq>) -> Response {
    if !st.multi_tenant { return StatusCode::NOT_FOUND.into_response(); }
    let Some(user_id) = session_user_id(&st, &headers).await else { return StatusCode::UNAUTHORIZED.into_response(); };
    // SECURITY: only switch to a tenant the user is a member of.
    match st.store.get_membership(user_id, TenantId(req.tenant_id)).await {
        Ok(Some(_)) => { set_session_tenant(&st, &headers, TenantId(req.tenant_id)).await; StatusCode::OK.into_response() }
        Ok(None) => StatusCode::FORBIDDEN.into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}
```
Register `.route("/admin/workspace/switch", post(admin_workspace_switch))`.

- [ ] **Step 4: Run test to verify it passes**

Run: `QUARK_TEST_DATABASE_URL=... CARGO_BUILD_JOBS=1 cargo test -j1 --test workspace_it switch`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -u && git commit -m "feat(api): POST /admin/workspace/switch — membership-checked current workspace"
```

---

### Task 5: `admin_guard` — cloud authorization by membership role

**Files:**
- Modify: `src/api.rs` (`admin_guard` OIDC-session branch)
- Test: extend `tests/tenant_enforcement.rs` (gated) + the existing status-contract test

**Interfaces:**
- Consumes: `get_membership`, `role_scopes`, `session.tenant_id`+`user_id`.
- Produces: in cloud mode, the OIDC-session branch of `admin_guard` derives `Principal.scopes` from `role_scopes(membership.role)` at `session.tenant_id`; no membership there → treated as insufficient (403 via the existing `saw_insufficient` path). OSS branch unchanged (session.scopes from the group→scope map).

- [ ] **Step 1: Write the failing test**

```rust
// tests/tenant_enforcement.rs (gated, cloud):
// - a session for a user who is Viewer in tenant T: admin_guard(required=LinksWrite) -> Err(FORBIDDEN);
//   admin_guard(required=LinksRead) -> Ok(Principal{ tenant: T, scopes has LinksRead }).
// - a session whose user has NO membership in session.tenant_id -> admin_guard(any) -> Err(FORBIDDEN).
// - OSS (multi_tenant=false): the existing group->scope behavior is unchanged (keep the existing test green).
```

- [ ] **Step 2: Run test to verify it fails**

Run: `QUARK_TEST_DATABASE_URL=... CARGO_BUILD_JOBS=1 cargo test -j1 --test tenant_enforcement guard_role`
Expected: FAIL — cloud still uses session.scopes (group→scope).

- [ ] **Step 3: Write minimal implementation**

In `admin_guard`'s OIDC-session branch (`src/api.rs:~1307`), where it currently does the covering check against `session.scopes`:
```rust
// OIDC session branch:
let effective_scopes = if st.multi_tenant {
    // Cloud: authorization comes from the user's role in the current workspace.
    match st.store.get_membership(session.user_id, session.tenant_id).await {
        Ok(Some(m)) => crate::tenant::role_scopes(m.role).to_vec(),
        Ok(None) => { saw_insufficient = true; vec![] }   // no membership in current tenant
        Err(_) => { saw_store_error = true; vec![] }
    }
} else {
    session.scopes.clone()   // OSS: unchanged (group->scope)
};
if effective_scopes.iter().any(|s| s.covers(required)) {
    return Ok(Principal { tenant: session.tenant_id, user_id: Some(session.user_id), scopes: effective_scopes });
}
if !effective_scopes.is_empty() { saw_insufficient = true; }
```
Preserve the surrounding `saw_insufficient`/`saw_rate_limited`/`saw_store_error`/`not_found_status` tail EXACTLY — only the scope source inside the covering check changes. Env-admin-token and API-token branches are untouched.

- [ ] **Step 4: Run test to verify it passes**

Run: `QUARK_TEST_DATABASE_URL=... CARGO_BUILD_JOBS=1 cargo test -j1 --test tenant_enforcement guard_role` + `CARGO_BUILD_JOBS=1 cargo test -j1` (OSS status-contract test unchanged)
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -u && git commit -m "feat(api): admin_guard authorizes by membership role in cloud (OSS unchanged)"
```

---

### Task 6: `/admin/me` returns memberships + current tenant

**Files:**
- Modify: `src/api.rs` (`admin_me` at :1618)
- Test: `tests/workspace_it.rs` (gated) + `web` types note (frontend consumes it in P2b-frontend)

**Interfaces:**
- Consumes: `list_memberships_for_user`, `get_tenant`, `session.tenant_id`+`user_id`.
- Produces: `admin_me` JSON gains `memberships: [{ tenant_id, name, slug, role }]` and `current_tenant: u64|null` (in cloud). Lets the frontend decide onboarding (0 memberships) vs switcher (≥1).

- [ ] **Step 1: Write the failing test**

```rust
// tests/workspace_it.rs (gated, cloud):
// - a fresh cloud user's /admin/me has memberships: [] and current_tenant: null -> signals onboarding.
// - after creating a workspace, /admin/me lists that membership (role Owner) and current_tenant == it.
```

- [ ] **Step 2: Run test to verify it fails**

Run: `QUARK_TEST_DATABASE_URL=... CARGO_BUILD_JOBS=1 cargo test -j1 --test workspace_it me_memberships`
Expected: FAIL — `me` has no memberships field.

- [ ] **Step 3: Write minimal implementation**

In `admin_me` (`src/api.rs:1618`), after resolving the session, add (cloud) the user's memberships (each joined with its tenant name/slug via `get_tenant`) and the current tenant:
```rust
// inside the authenticated session branch:
let memberships = if st.multi_tenant {
    let ms = st.store.list_memberships_for_user(session.user_id).await.unwrap_or_default();
    let mut out = Vec::new();
    for m in ms {
        if let Ok(Some(t)) = st.store.get_tenant(m.tenant_id).await {
            out.push(serde_json::json!({ "tenant_id": t.id.0, "name": t.name, "slug": t.slug, "role": m.role }));
        }
    }
    out
} else { Vec::new() };
// add to the JSON: "memberships": memberships, "current_tenant": st.multi_tenant.then_some(session.tenant_id.0)
```
Keep the existing `authenticated`/`oidc` fields.

- [ ] **Step 4: Run test to verify it passes**

Run: `QUARK_TEST_DATABASE_URL=... CARGO_BUILD_JOBS=1 cargo test -j1 --test workspace_it me_memberships`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -u && git commit -m "feat(api): /admin/me returns memberships + current tenant (cloud)"
```

---

### Task 7: OSS parity + security sweep

**Files:**
- Modify: `tests/workspace_it.rs`, `tests/tenant_enforcement.rs`
- Test: whole suite

**Interfaces:** Consumes everything above.

- [ ] **Step 1: Add assertions**

```rust
// - OSS parity: with multi_tenant=false, /admin/tenants and /admin/workspace/switch are 404;
//   login creates the tenant-0 membership; admin_guard uses group->scope; full pre-existing suite green.
// - Security: switch to a non-member tenant is 403 and does NOT mutate the session (re-read /admin/me
//   shows the original current_tenant); cloud login leaves 0 memberships (no tenant-0 leak).
```

- [ ] **Step 2: Run — OSS parity (flag off)**

Run: `CARGO_BUILD_JOBS=1 cargo test -j1`
Expected: full pre-existing suite PASSES unchanged (Postgres-gated tests skip without the env).

- [ ] **Step 3: Commit**

```bash
git add -u && git commit -m "test(workspace): OSS parity + switch/login security sweep"
```

---

## Self-Review

**1. Spec coverage:** tenant-id allocator → Task 1; cloud login no auto-membership → Task 2; create workspace → Task 3; switch (membership-checked) → Task 4; cloud auth by role → Task 5; /admin/me memberships+current → Task 6; OSS parity + security → Tasks 5/7. ✓ All spec sections covered. (UI deferred to P2b-frontend, per spec.)

**2. Placeholder scan:** Test bodies in Tasks 3/4/6/7 describe assertions in prose because they hook the existing gated OIDC-session harness (fixtures live in the repo); the implementer wires them. Production-code steps carry concrete code. The `session_user_id`/`set_session_tenant`/`conflict_or_503` helpers are introduced in Task 3 with signatures and reused in Tasks 4/6 — not placeholders.

**3. Type consistency:** `next_tenant_id() -> u64`; `Tenant{id:TenantId,name,slug,created}`; `Membership{user_id,tenant_id,role,created}` with `Role::Owner`; `ensure_user_and_membership(store, multi_tenant, subject, email, display, scopes)`; `session_user_id`/`set_session_tenant` used consistently across Tasks 3-6; `admin_guard` still returns `Result<Principal,StatusCode>` with the tail preserved. Security invariants (switch verifies membership; cloud no tenant-0 membership; no-membership→403) appear in the tasks that own them.
