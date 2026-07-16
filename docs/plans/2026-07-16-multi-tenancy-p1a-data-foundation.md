# Multi-tenancy P1a — Data Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every piece of tenant-owned data carry a `tenant_id` and be accessed only through a tenant-scoped handle, across both the Postgres and LMDB backends, with existing data migrated into a default tenant (`0`) — so the OSS binary behaves identically to today while the cloud foundation is in place.

**Architecture:** New `Tenant`/`User`/`Membership` domain types; the `Store` trait's tenant-owned methods gain a `tenant: TenantId` first parameter, and a `ScopedStore` wrapper (`store.for_tenant(tid)`) exposes those methods with the tenant captured so call sites can't forget it. Postgres gets a `tenant_id` column on every tenant-owned table plus new identity tables and an idempotent migration; LMDB prefixes every tenant-owned key with the big-endian `tenant_id`. Postgres RLS is defined and enabled only in cloud mode as fail-closed defense-in-depth.

**Tech Stack:** Rust 2021, `async-trait`, `sqlx` (Postgres, `tls-rustls`), `heed` (LMDB), `tokio`.

## Global Constraints

- All code and comments in English.
- One binary serves both modes: OSS pins every scoped call to `TenantId(0)`; cloud resolves the tenant per request (auth resolution is P1b — out of scope here).
- The short-code namespace stays **global**: `src/codec.rs` and `src/permute.rs` are NOT modified. `tenant_id` is an ownership column, never a code-space partition.
- Postgres-backed tests are gated behind `QUARK_TEST_DATABASE_URL` (skipped when unset), mirroring existing gated tests.
- Rust tests in this environment run with `CARGO_BUILD_JOBS=1` and `cargo test -j1` (linker constraint).
- Reuse existing patterns: the lease pattern (`try_acquire_health_lease`), the `Store` seam, existing gated-test helpers.
- Out of scope for P1a (do NOT implement): `admin_guard`/token/session carrying the tenant, the `QUARK_MULTI_TENANT` flag wiring into request handling (P1b); signup/invites (P2); `Host→tenant`/custom domains (P3); per-tenant Sheets/ClickHouse (P4).

## File Structure

- **Create** `src/tenant.rs` — `TenantId`, `Tenant`, `User`, `Role`, `Membership`, `role_scopes()`, `DEFAULT_TENANT`. One responsibility: the tenancy domain model.
- **Modify** `src/lib.rs` — register `pub mod tenant;`.
- **Modify** `src/store/mod.rs` — add `tenant: TenantId` to tenant-owned trait methods; add `ScopedStore` + `Store::for_tenant`; new trait methods `put_tenant`/`get_tenant`, `put_user`/`get_user_by_subject`, `put_membership`/`list_memberships_for_user`/`get_membership`.
- **Modify** `src/store/postgres.rs` — `init_schema` DDL (new tables, `tenant_id` columns, indexes, RLS), migration, tenant-scoped query bodies, identity-table methods.
- **Modify** `src/store/lmdb.rs` — key-prefix helper, prefixed keys on every tenant-owned sub-db, bounded range scans, boot re-keying migration, identity-table sub-dbs.
- **Modify** `src/api.rs` / `src/main.rs` — rewire call sites to `store.for_tenant(DEFAULT_TENANT)` (mechanical; no behavior change).
- **Test** `tests/tenant_isolation.rs` (new) — cross-tenant isolation over both backends + OSS parity.

---

### Task 1: Core tenancy types

**Files:**
- Create: `src/tenant.rs`
- Modify: `src/lib.rs` (add `pub mod tenant;`)
- Test: inline `#[cfg(test)]` in `src/tenant.rs`

**Interfaces:**
- Produces: `TenantId(pub u64)`, `const DEFAULT_TENANT: TenantId = TenantId(0)`; structs `Tenant { id: TenantId, name: String, slug: String, created: u64 }`, `User { id: u64, subject: String, email: String, display: String, created: u64 }`, `Membership { user_id: u64, tenant_id: TenantId, role: Role, created: u64 }`; `enum Role { Owner, Admin, Member }`; `fn role_scopes(role: Role) -> &'static [crate::auth::Scope]`.

- [ ] **Step 1: Write the failing test**

```rust
// in src/tenant.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Scope;

    #[test]
    fn default_tenant_is_zero() {
        assert_eq!(DEFAULT_TENANT, TenantId(0));
    }

    #[test]
    fn owner_covers_full() {
        assert!(role_scopes(Role::Owner).contains(&Scope::Full));
    }

    #[test]
    fn member_can_write_and_read_but_not_full() {
        let s = role_scopes(Role::Member);
        assert!(s.contains(&Scope::LinksWrite));
        assert!(s.contains(&Scope::LinksRead));
        assert!(s.contains(&Scope::Analytics));
        assert!(!s.contains(&Scope::Full));
    }

    #[test]
    fn tenant_id_roundtrips_through_json() {
        let t = TenantId(42);
        let j = serde_json::to_string(&t).unwrap();
        assert_eq!(serde_json::from_str::<TenantId>(&j).unwrap(), t);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_BUILD_JOBS=1 cargo test -j1 --lib tenant::tests`
Expected: FAIL — `src/tenant.rs` / module does not exist.

- [ ] **Step 3: Write minimal implementation**

```rust
// src/tenant.rs
//! Tenancy domain model. A Tenant owns all data; a User is a global identity;
//! a Membership links a User to a Tenant with a Role. In OSS mode exactly one
//! tenant exists (`DEFAULT_TENANT`); cloud mode has many.
use serde::{Deserialize, Serialize};
use crate::auth::Scope;

/// Opaque tenant identifier. `0` is the default/OSS tenant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TenantId(pub u64);

/// The single implicit tenant in OSS mode, and the tenant existing data is
/// migrated into.
pub const DEFAULT_TENANT: TenantId = TenantId(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tenant {
    pub id: TenantId,
    pub name: String,
    pub slug: String,
    pub created: u64,
}

/// A global user identity, keyed by the OIDC `subject` (immutable), never email.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: u64,
    pub subject: String,
    pub email: String,
    pub display: String,
    pub created: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    Admin,
    Member,
}

/// Many-to-many join between a user and a tenant, carrying the role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Membership {
    pub user_id: u64,
    pub tenant_id: TenantId,
    pub role: Role,
    pub created: u64,
}

/// Maps a role to the permission scopes it grants. Kept as a function (not a
/// stored set) so roles can be split/extended later without a schema change.
pub fn role_scopes(role: Role) -> &'static [Scope] {
    match role {
        // Owner/Admin are superusers within their tenant (tenant-management
        // authorization — deleting/transferring the tenant — is enforced at the
        // handler layer in P2, not via a scope).
        Role::Owner | Role::Admin => &[Scope::Full],
        Role::Member => &[Scope::LinksWrite, Scope::LinksRead, Scope::Analytics],
    }
}
```

Add to `src/lib.rs` (alongside the other `pub mod` lines):

```rust
pub mod tenant;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `CARGO_BUILD_JOBS=1 cargo test -j1 --lib tenant::tests`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add src/tenant.rs src/lib.rs
git commit -m "feat(tenant): core tenancy types (Tenant/User/Membership/Role)"
```

---

### Task 2: Store trait tenant-scoping + `ScopedStore` handle

**Files:**
- Modify: `src/store/mod.rs:283-481` (trait), and the call sites in `src/api.rs`, `src/main.rs`
- Test: inline `#[cfg(test)]` in `src/store/mod.rs` (ScopedStore delegation, backend-agnostic via LMDB)

**Interfaces:**
- Consumes: `TenantId`, `DEFAULT_TENANT` from Task 1.
- Produces:
  - Every **tenant-owned** `Store` method gains `tenant: TenantId` as its first parameter. The exact list (from the audit) is: `next_id`, `get_link`, `put_link`, `get_alias`, `put_alias_and_link`, `put_link_tx`, `put_alias_and_link_tx`, `delete_link_tx`, `list_links`, `search_links`, `list_aliases`, `list_tags`, `list_folders`, `delete_link`, `delete_alias`, `list_webhooks`, `get_webhook`, `put_webhook`, `delete_webhook`, `next_webhook_id`, `list_api_tokens`, `put_api_token`, `delete_api_token`, `next_api_token_id`, `put_session`, `bump_visits`, `visits`, `put_link_health`, `list_link_health`, `link_health_for`, `list_broken_link_ids`, `put_sheets_connection`, `get_sheets_connection`, `delete_sheets_connection`, `next_pixel_id`, `get_pixel`, `put_pixel`, `delete_pixel`, `list_pixels`, `get_wellknown`, `put_wellknown`, `delete_wellknown`.
  - **Global/infra** methods stay UNCHANGED (no tenant param): `gc_sessions`, `try_acquire_health_lease`, `try_acquire_sheets_lease`, `enqueue_deliveries`, `claim_due_deliveries`, `mark_delivered`, `mark_retry`, `mark_dead`. Also **hash-lookup** methods stay tenant-less and RETURN the tenant on the row: `get_api_token_by_hash`, `get_session_by_hash`, `delete_session`.
  - New identity methods on `Store` (tenant-less; they manage tenancy itself): `put_tenant(&Tenant)`, `get_tenant(TenantId) -> Option<Tenant>`, `put_user(&User)`, `get_user_by_subject(&str) -> Option<User>`, `next_user_id() -> u64`, `put_membership(&Membership)`, `get_membership(user_id: u64, tenant: TenantId) -> Option<Membership>`, `list_memberships_for_user(user_id: u64) -> Vec<Membership>`.
  - `Store::for_tenant(self: &Arc<Self>, tenant: TenantId) -> ScopedStore` and struct `ScopedStore { inner: Arc<dyn Store>, tenant: TenantId }` whose inherent methods mirror the tenant-owned list above WITHOUT the `tenant` param.

> Note on `Session`/`ApiToken` carrying the tenant: the *struct* field changes (`Session.tenant_id`, `ApiToken.tenant_id`) are part of P1b (auth binding). In P1a, `put_session`/`put_api_token` receive `tenant` as a parameter and the backend stores it in a column/prefix; the returned struct from the hash-lookups does not yet expose it. This keeps P1a a pure data-layer change.

- [ ] **Step 1: Write the failing test** (ScopedStore delegates with the captured tenant; two tenants are isolated on LMDB)

```rust
// in src/store/mod.rs
#[cfg(test)]
mod scoped_tests {
    use super::*;
    use crate::tenant::TenantId;

    #[tokio::test]
    async fn scoped_store_isolates_links_by_tenant() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path()).await.unwrap();
        let a = store.clone().for_tenant(TenantId(1));
        let b = store.clone().for_tenant(TenantId(2));

        let rec = crate::store::Record::for_test("https://example.com");
        a.put_link(100, &rec).await.unwrap();

        assert!(a.get_link(100).await.unwrap().is_some());
        // Tenant 2 must NOT see tenant 1's link at the same id.
        assert!(b.get_link(100).await.unwrap().is_none());
    }
}
```

(If `Record::for_test` does not exist, construct a `Record` inline using the existing public constructor/fields visible in `src/store/mod.rs`; the reviewer should use whatever the current `Record` shape requires.)

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_BUILD_JOBS=1 cargo test -j1 --lib store::scoped_tests`
Expected: FAIL — `for_tenant` does not exist / signature mismatch.

- [ ] **Step 3: Write minimal implementation**

In `src/store/mod.rs`, add `tenant: TenantId` as the first parameter to each tenant-owned method listed in Interfaces. Example transformation (apply the same shape to every method in the list):

```rust
// before
async fn get_link(&self, id: u64) -> Result<Option<Record>, StoreError>;
// after
async fn get_link(&self, tenant: TenantId, id: u64) -> Result<Option<Record>, StoreError>;
```

Add the new identity methods to the trait:

```rust
async fn put_tenant(&self, t: &crate::tenant::Tenant) -> Result<(), StoreError>;
async fn get_tenant(&self, id: crate::tenant::TenantId) -> Result<Option<crate::tenant::Tenant>, StoreError>;
async fn next_user_id(&self) -> Result<u64, StoreError>;
async fn put_user(&self, u: &crate::tenant::User) -> Result<(), StoreError>;
async fn get_user_by_subject(&self, subject: &str) -> Result<Option<crate::tenant::User>, StoreError>;
async fn put_membership(&self, m: &crate::tenant::Membership) -> Result<(), StoreError>;
async fn get_membership(&self, user_id: u64, tenant: crate::tenant::TenantId)
    -> Result<Option<crate::tenant::Membership>, StoreError>;
async fn list_memberships_for_user(&self, user_id: u64)
    -> Result<Vec<crate::tenant::Membership>, StoreError>;
```

Add the wrapper + constructor (at module level, after the trait):

```rust
use crate::tenant::TenantId;

/// A tenant-scoped view over a `Store`. Its methods mirror the tenant-owned
/// `Store` methods but capture the tenant, so a call site cannot forget it.
pub struct ScopedStore {
    inner: std::sync::Arc<dyn Store>,
    tenant: TenantId,
}

impl dyn Store {
    /// Returns a handle bound to `tenant`. All tenant-owned reads/writes go
    /// through it.
    pub fn for_tenant(self: std::sync::Arc<Self>, tenant: TenantId) -> ScopedStore {
        ScopedStore { inner: self, tenant }
    }
}

impl ScopedStore {
    pub fn tenant(&self) -> TenantId { self.tenant }

    pub async fn get_link(&self, id: u64) -> Result<Option<Record>, StoreError> {
        self.inner.get_link(self.tenant, id).await
    }
    pub async fn put_link(&self, id: u64, rec: &Record) -> Result<(), StoreError> {
        self.inner.put_link(self.tenant, id, rec).await
    }
    // ... one thin delegate per tenant-owned method in the Interfaces list,
    // each forwarding `self.tenant` as the first argument.
}
```

Then update every call site. In P1a all handlers use the default tenant (real resolution is P1b). Two mechanical patterns:

```rust
// api.rs handler bodies: replace `st.store.get_link(id)` with a scoped handle
let store = st.store.clone().for_tenant(crate::tenant::DEFAULT_TENANT);
let rec = store.get_link(id).await?;
```

```rust
// main.rs background workers (health checker, sheets sync): same — obtain
// `store.clone().for_tenant(DEFAULT_TENANT)` once at the top of the loop body
// for tenant-owned calls; leave lease/gc/outbox calls on the bare `store`.
```

Global/infra and hash-lookup call sites (`gc_sessions`, leases, `claim_due_deliveries`, `mark_*`, `get_api_token_by_hash`, `get_session_by_hash`, `delete_session`, `enqueue_deliveries`) stay on the bare `store` and are unchanged.

Both backends' `impl Store` (Postgres, LMDB) get the new method signatures — Tasks 3–5 fill in the bodies. For this task to compile, add the `tenant`/identity parameters to the impls and forward to a `todo!()`-free minimal body **only if needed to compile**; prefer to land Task 2 together with Tasks 4/5's signatures so the tree compiles. (Practically: do the trait edit, the ScopedStore, and the backend signature updates with real bodies in the same commit as Tasks 4–5 if the borrow checker forces it; the reviewer keeps the tree green.)

- [ ] **Step 4: Run test to verify it passes**

Run: `CARGO_BUILD_JOBS=1 cargo test -j1 --lib store::scoped_tests`
Expected: PASS. Also `CARGO_BUILD_JOBS=1 cargo build -j1` compiles the whole tree.

- [ ] **Step 5: Commit**

```bash
git add src/store/mod.rs src/api.rs src/main.rs
git commit -m "feat(store): tenant-scoped trait methods + ScopedStore handle"
```

---

### Task 3: Postgres schema — identity tables, tenant_id columns, migration

**Files:**
- Modify: `src/store/postgres.rs` (`init_schema` DDL; the `reset_for_tests` TRUNCATE list)
- Test: `tests/tenant_isolation.rs` migration assertion (gated by `QUARK_TEST_DATABASE_URL`)

**Interfaces:**
- Consumes: types from Task 1.
- Produces: tables `tenants`, `users`, `memberships`; a `tenant_id BIGINT NOT NULL DEFAULT 0` column on every tenant-owned table; adjusted indexes; a seeded row `tenants(0,'default','default')`. Idempotent (safe to run on an existing DB and on every boot).

- [ ] **Step 1: Write the failing test**

```rust
// tests/tenant_isolation.rs
#[tokio::test]
async fn migration_seeds_default_tenant_and_columns() {
    let Some(url) = std::env::var("QUARK_TEST_DATABASE_URL").ok() else { return; };
    let store = quark::store::open_postgres(&url).await.unwrap(); // existing constructor
    // default tenant exists after init_schema
    let t = store.get_tenant(quark::tenant::TenantId(0)).await.unwrap();
    assert!(t.is_some(), "default tenant 0 must be seeded by init_schema");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `QUARK_TEST_DATABASE_URL=... CARGO_BUILD_JOBS=1 cargo test -j1 --test tenant_isolation migration_seeds`
Expected: FAIL — `get_tenant` returns `None` / table missing.

- [ ] **Step 3: Write minimal implementation**

Append to `init_schema` (after the existing `CREATE TABLE` block). All statements use `IF NOT EXISTS` / `ADD COLUMN IF NOT EXISTS` so re-running is a no-op:

```sql
CREATE TABLE IF NOT EXISTS tenants (
    id BIGINT PRIMARY KEY,
    name TEXT NOT NULL,
    slug TEXT NOT NULL UNIQUE,
    created BIGINT NOT NULL
);
INSERT INTO tenants (id, name, slug, created)
    VALUES (0, 'default', 'default', 0)
    ON CONFLICT (id) DO NOTHING;

CREATE TABLE IF NOT EXISTS users (
    id BIGINT PRIMARY KEY,
    subject TEXT NOT NULL UNIQUE,
    email TEXT NOT NULL,
    display TEXT NOT NULL,
    created BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS memberships (
    user_id BIGINT NOT NULL,
    tenant_id BIGINT NOT NULL,
    role TEXT NOT NULL,
    created BIGINT NOT NULL,
    PRIMARY KEY (user_id, tenant_id)
);
CREATE INDEX IF NOT EXISTS memberships_by_tenant ON memberships (tenant_id);
```

Add a `tenant_id` column to each tenant-owned table (existing rows default to `0` = the seeded default tenant). Exact table list from the audit — `links`, `aliases`, `link_health`, `sessions`, `webhooks`, `api_tokens`, `pixels`, `wellknown_documents`, `click_counters`, `stats_meta`, `click_events`, `webhook_deliveries`, `sheets_connection`:

```sql
ALTER TABLE links ADD COLUMN IF NOT EXISTS tenant_id BIGINT NOT NULL DEFAULT 0;
ALTER TABLE aliases ADD COLUMN IF NOT EXISTS tenant_id BIGINT NOT NULL DEFAULT 0;
-- ...repeat the ALTER for every table in the list above...
ALTER TABLE sheets_connection ADD COLUMN IF NOT EXISTS tenant_id BIGINT NOT NULL DEFAULT 0;
```

Rework the `sheets_connection` singleton: drop the `singleton BOOLEAN PRIMARY KEY` semantics by making the effective key `(tenant_id)`:

```sql
CREATE UNIQUE INDEX IF NOT EXISTS sheets_connection_by_tenant ON sheets_connection (tenant_id);
```

Add per-tenant listing/aggregation indexes (the audit-flagged ones):

```sql
CREATE INDEX IF NOT EXISTS links_by_tenant_id ON links (tenant_id, id);
CREATE INDEX IF NOT EXISTS webhooks_by_tenant ON webhooks (tenant_id, id);
CREATE INDEX IF NOT EXISTS pixels_by_tenant ON pixels (tenant_id, id);
CREATE INDEX IF NOT EXISTS api_tokens_by_tenant ON api_tokens (tenant_id, id);
CREATE INDEX IF NOT EXISTS click_counters_by_tenant ON click_counters (tenant_id, id, dimension, bucket);
```

Update `reset_for_tests` to also `TRUNCATE tenants, users, memberships` and re-seed tenant 0 (so gated tests start clean but with the default tenant present).

- [ ] **Step 4: Run test to verify it passes**

Run: `QUARK_TEST_DATABASE_URL=... CARGO_BUILD_JOBS=1 cargo test -j1 --test tenant_isolation migration_seeds`
Expected: PASS. Re-run once more to confirm idempotency (no error on second `init_schema`).

- [ ] **Step 5: Commit**

```bash
git add src/store/postgres.rs tests/tenant_isolation.rs
git commit -m "feat(store/pg): identity tables + tenant_id columns + idempotent migration"
```

---

### Task 4: Postgres tenant-scoped queries + RLS

**Files:**
- Modify: `src/store/postgres.rs` (`impl Store` method bodies; a `SET LOCAL` transaction helper; RLS policies in `init_schema`; the new identity-method bodies)
- Test: `tests/tenant_isolation.rs` (Postgres arm, gated)

**Interfaces:**
- Consumes: the trait signatures from Task 2, the schema from Task 3.
- Produces: every tenant-owned Postgres method filters/writes by the `tenant` argument; identity methods implemented; RLS enabled in cloud mode.

- [ ] **Step 1: Write the failing test** (cross-tenant isolation on Postgres)

```rust
// tests/tenant_isolation.rs
#[tokio::test]
async fn pg_two_tenants_do_not_see_each_others_links() {
    let Some(url) = std::env::var("QUARK_TEST_DATABASE_URL").ok() else { return; };
    let store = quark::store::open_postgres(&url).await.unwrap();
    store.reset_for_tests().await.unwrap();
    let a = store.clone().for_tenant(quark::tenant::TenantId(1));
    let b = store.clone().for_tenant(quark::tenant::TenantId(2));
    let rec = /* build a Record for https://example.com */;
    a.put_link(500, &rec).await.unwrap();
    assert!(a.get_link(500).await.unwrap().is_some());
    assert!(b.get_link(500).await.unwrap().is_none());
    assert_eq!(b.list_links(None, 100, None, None).await.unwrap().len(), 0);
    assert_eq!(a.list_links(None, 100, None, None).await.unwrap().len(), 1);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `QUARK_TEST_DATABASE_URL=... CARGO_BUILD_JOBS=1 cargo test -j1 --test tenant_isolation pg_two_tenants`
Expected: FAIL — methods ignore tenant (b sees a's link) until scoping is added.

- [ ] **Step 3: Write minimal implementation**

For every tenant-owned method, add `tenant_id` to the WHERE clause (reads/deletes/updates) and to the column list (inserts). Representative examples (apply the same to every tenant-owned method):

```rust
// get_link
async fn get_link(&self, tenant: TenantId, id: u64) -> Result<Option<Record>, StoreError> {
    let row = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT record FROM links WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant.0 as i64)
    .bind(id as i64)
    .fetch_optional(&self.read)   // read pool (unchanged split)
    .await?;
    // ...deserialize as today...
}

// put_link (insert/upsert): add tenant_id to columns + ON CONFLICT target stays (tenant_id, id)-aware
async fn put_link(&self, tenant: TenantId, id: u64, rec: &Record) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO links (tenant_id, id, record) VALUES ($1, $2, $3)
         ON CONFLICT (id) DO UPDATE SET record = EXCLUDED.record, tenant_id = EXCLUDED.tenant_id",
    )
    .bind(tenant.0 as i64).bind(id as i64).bind(serde_json::to_value(rec)?)
    .execute(&self.write).await?;
    Ok(())
}

// list_links: existing keyset query gains `tenant_id = $` as the leading predicate.
```

Implement the identity methods against the Task 3 tables (`put_tenant`/`get_tenant`/`put_user`/`get_user_by_subject`/`next_user_id`/`put_membership`/`get_membership`/`list_memberships_for_user`) with straight `INSERT ... ON CONFLICT` / `SELECT` bodies; `next_user_id` uses a `quark_user_id_seq` sequence created in `init_schema` (`CREATE SEQUENCE IF NOT EXISTS quark_user_id_seq;`).

RLS — add to `init_schema`, and a per-transaction guard used ONLY in cloud mode:

```sql
ALTER TABLE links ENABLE ROW LEVEL SECURITY;
CREATE POLICY links_tenant_isolation ON links
    USING (tenant_id = current_setting('app.tenant_id', true)::bigint);
-- repeat ENABLE + POLICY for every tenant-owned table
```

```rust
// A cloud-mode transaction sets the tenant for RLS. `SET LOCAL` scopes it to
// the transaction so a pooled connection never leaks the previous tenant.
// In OSS mode (single tenant) RLS is left disabled and this is not called.
async fn begin_tenant_tx(&self, tenant: TenantId) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, StoreError> {
    let mut tx = self.write.begin().await?;
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(tenant.0.to_string())
        .execute(&mut *tx).await?;
    Ok(tx)
}
```

Gate RLS enablement: enable the policies' enforcement only when cloud mode is active. For P1a, keep RLS policies **created but permissive by default** by NOT calling `FORCE ROW LEVEL SECURITY` and relying on the app-level `WHERE tenant_id` as the enforced layer; the `begin_tenant_tx` harness + `FORCE` is wired by P1b's mode flag. Document this in a comment so the reviewer knows RLS is defined here but activated in P1b.

- [ ] **Step 4: Run test to verify it passes**

Run: `QUARK_TEST_DATABASE_URL=... CARGO_BUILD_JOBS=1 cargo test -j1 --test tenant_isolation pg_two_tenants`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/store/postgres.rs tests/tenant_isolation.rs
git commit -m "feat(store/pg): tenant-scoped queries + RLS policies (activation in P1b)"
```

---

### Task 5: LMDB key-prefixing + boot re-keying migration

**Files:**
- Modify: `src/store/lmdb.rs` (key helpers; prefixed keys on tenant-owned sub-dbs; bounded scans; identity sub-dbs; boot re-keying)
- Test: `tests/tenant_isolation.rs` (LMDB arm — no gating)

**Interfaces:**
- Consumes: trait signatures from Task 2.
- Produces: every tenant-owned LMDB key is prefixed with the big-endian `tenant_id`; range scans are bounded to the tenant prefix; identity data (`tenants`/`users`/`memberships`) persisted; pre-tenancy data re-keyed to tenant 0 on boot.

- [ ] **Step 1: Write the failing test** (already covered by Task 2's `scoped_store_isolates_links_by_tenant`, extend to lists + re-keying)

```rust
// tests/tenant_isolation.rs
#[tokio::test]
async fn lmdb_scans_are_bounded_to_tenant() {
    let dir = tempfile::tempdir().unwrap();
    let store = quark::store::open_store(dir.path()).await.unwrap();
    let a = store.clone().for_tenant(quark::tenant::TenantId(1));
    let b = store.clone().for_tenant(quark::tenant::TenantId(2));
    let rec = /* build Record */;
    a.put_link(1, &rec).await.unwrap();
    a.put_link(2, &rec).await.unwrap();
    b.put_link(3, &rec).await.unwrap();
    assert_eq!(a.list_links(None, 100, None, None).await.unwrap().len(), 2);
    assert_eq!(b.list_links(None, 100, None, None).await.unwrap().len(), 1);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_BUILD_JOBS=1 cargo test -j1 --test tenant_isolation lmdb_scans`
Expected: FAIL — scans return all rows regardless of tenant.

- [ ] **Step 3: Write minimal implementation**

Add a key-prefix helper and apply it to every tenant-owned sub-db (the audit's 11 tenant-owned keyspaces: `links`, `aliases`, `stats`, `events`, `webhooks`, `api_tokens`, `visits`, `pixels`, `wellknown`, `health`, `sheets`; plus counters in `meta`):

```rust
/// Prefixes a big-endian tenant id onto a key so each tenant occupies a
/// disjoint, contiguous range within a shared sub-db.
fn tkey(tenant: TenantId, key: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + key.len());
    out.extend_from_slice(&tenant.0.to_be_bytes());
    out.extend_from_slice(key);
    out
}
```

- For fixed-key sub-dbs (`Str`/`BeU64`), switch the sub-db key type to `Bytes` and store `tkey(tenant, original_key_bytes)`.
- For range scans (`list_links`, `list_tags`, `list_folders`, `list_aliases`, iterators), replace the unbounded iterator with a range bounded to `[tkey(tenant, MIN) .. tkey(tenant, MAX)]` — i.e. iterate the 8-byte tenant prefix range only, then strip the prefix when decoding the stored key. Keyset pagination (`after: id`) starts at `tkey(tenant, after)`.
- Hash-lookup sub-dbs (`sessions`, `api_tokens` by hash): keep a global lookup path, but store the tenant inside the value (already handled by the struct in P1b) or as part of a secondary prefixed key. For P1a, keep `api_tokens`/`sessions` keyed by hash globally and store `tenant_id` alongside the JSON value.
- `meta` counters (`next_id`, `next_webhook_id`, `next_api_token_id`, `next_pixel_id`): stay **global** (the id/code namespace is global per Global Constraints) — do NOT prefix these.
- Add three identity sub-dbs — bump `MAX_DBS` from 13 to 16 — `tenants` (key `BeU64` id → JSON), `users` (key `Str` subject → JSON, plus a `user_by_id` need is avoided by storing id in the value), `memberships` (key = `user_id` be + `tenant_id` be → JSON). Implement `next_user_id` via a `meta["next_user_id"]` counter.

Boot re-keying migration (runs once, under the existing lease pattern so multi-node is safe):

```rust
/// One-time migration: any tenant-owned key written before tenancy has no
/// 8-byte prefix. On boot, re-key such entries under DEFAULT_TENANT. Guarded by
/// a `meta["tenancy_migrated"]` marker so it runs at most once.
async fn migrate_pre_tenancy_keys_to_default(&self) -> Result<(), StoreError> {
    // if meta["tenancy_migrated"] == 1 -> return early.
    // else: for each tenant-owned sub-db, for each key whose length shows it is
    // un-prefixed (old fixed width), read value, delete old key, write
    // tkey(DEFAULT_TENANT, old_key) -> value, in a write txn. Set marker.
}
```

Call `migrate_pre_tenancy_keys_to_default` from the LMDB store constructor (after `init`), matching where the health/sheets setup runs; on a fresh DB it is a no-op (nothing to migrate) and sets the marker.

- [ ] **Step 4: Run test to verify it passes**

Run: `CARGO_BUILD_JOBS=1 cargo test -j1 --test tenant_isolation lmdb_scans`
Expected: PASS. Also `scoped_store_isolates_links_by_tenant` (Task 2) still PASS.

- [ ] **Step 5: Commit**

```bash
git add src/store/lmdb.rs tests/tenant_isolation.rs
git commit -m "feat(store/lmdb): tenant key-prefixing + bounded scans + boot re-keying"
```

---

### Task 6: Full cross-tenant isolation + OSS parity

**Files:**
- Modify: `tests/tenant_isolation.rs` (broaden coverage to every tenant-owned method)
- Test: the whole existing suite for OSS parity

**Interfaces:**
- Consumes: everything above.
- Produces: a test that exercises **every** tenant-owned method for cross-tenant leakage on both backends, and confirms the existing suite is unchanged under the default tenant.

- [ ] **Step 1: Write the failing/あregression test**

```rust
// tests/tenant_isolation.rs
// A table-driven test: for each tenant-owned entity (link, alias, webhook,
// api_token, pixel, wellknown, health, visits, sheets_connection), tenant A
// writes one and tenant B must read zero. Runs on LMDB always, and on Postgres
// when QUARK_TEST_DATABASE_URL is set (loop over both stores).
#[tokio::test]
async fn every_tenant_owned_entity_is_isolated() {
    // build store(s); for each entity: a.put_*(...); assert b.get_*/list_* empty; assert a sees it.
    // (Full body enumerates each method pair from the Task 2 Interfaces list.)
}
```

- [ ] **Step 2: Run test to verify it fails (then passes as impl lands)**

Run: `CARGO_BUILD_JOBS=1 cargo test -j1 --test tenant_isolation`
Expected: any method that forgot the prefix/WHERE fails here — fix it in the owning task, then this passes.

- [ ] **Step 3: Confirm OSS parity — the existing suite is unchanged**

Run: `CARGO_BUILD_JOBS=1 cargo test -j1`
Expected: the pre-existing lib (212) + api_it (87) tests PASS unchanged — the default-tenant path is behavior-preserving. (Postgres-gated tests only run with `QUARK_TEST_DATABASE_URL`.)

- [ ] **Step 4: Commit**

```bash
git add tests/tenant_isolation.rs
git commit -m "test(tenant): cross-tenant isolation over both backends + OSS parity"
```

---

## Self-Review

**1. Spec coverage** (against `2026-07-16-multi-tenancy-p1-foundation-design.md`):
- Entities Tenant/User/Membership + roles → Task 1. ✓
- `tenant_id` on all tenant-owned data, both backends → Tasks 3 (PG schema), 4 (PG queries), 5 (LMDB). ✓
- ScopedStore handle → Task 2. ✓
- RLS defined, cloud-only activation → Task 4 (defined; activation deferred to P1b, documented). ✓
- Migration to tenant 0 → Task 3 (PG), Task 5 (LMDB boot re-keying). ✓
- Isolation + OSS parity tests → Tasks 4, 5, 6. ✓
- Code namespace stays global (codec/permute untouched) → Global Constraints + Task 5 keeps `meta` counters global. ✓
- Out-of-scope items (auth binding, mode flag, signup, domains, Sheets/ClickHouse per tenant) → not present. ✓

**2. Placeholder scan:** No "TBD"/"implement later". Two intentional deferrals are explicit and scoped (RLS activation → P1b; `Session`/`ApiToken` struct field → P1b), not vague placeholders. The `Record` construction in tests defers to the current `Record` shape — flagged for the implementer because the exact constructor isn't visible in the audited excerpts.

**3. Type consistency:** `TenantId(u64)`, `DEFAULT_TENANT`, `Role`, `role_scopes` used identically across tasks. Tenant-owned method list in Task 2 matches the columns altered in Task 3 and the keyspaces prefixed in Task 5. Global/infra + hash-lookup exclusions are consistent across Tasks 2, 4, 5.
