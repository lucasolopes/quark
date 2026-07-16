# Multi-tenancy P2a — Enforcement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** In cloud mode (`QUARK_MULTI_TENANT=1`), make a tenant ≠ 0 truly safe by enforcing Postgres `FORCE ROW LEVEL SECURITY` with per-transaction `SET LOCAL app.tenant_id`, and close the last 4 code paths still pinned to `DEFAULT_TENANT` — while OSS mode (default) stays byte-for-byte the same.

**Architecture:** A boot flag threads `multi_tenant: bool` into `AppState` and `PostgresStore`. Every tenant-owned Postgres method routes through `with_read`/`with_write` helpers: in cloud mode they run the query inside a transaction that first does `SET LOCAL app.tenant_id`; in OSS mode they run directly on the pool (no tx). `init_schema` issues `FORCE ROW LEVEL SECURITY` (cloud only) on 11 tenant-owned tables — `api_tokens`/`sessions` are excepted so their by-hash lookups (which don't know the tenant up front) still work. Four `DEFAULT_TENANT` hardcodes get a real tenant threaded through.

**Tech Stack:** Rust 2021, axum, sqlx (Postgres), tokio.

## Global Constraints

- All code and comments in English.
- OSS mode (`QUARK_MULTI_TENANT` unset/`0`) is unchanged: no FORCE, no tenant-tx, everything `DEFAULT_TENANT`; the existing test suite passes identically.
- Cloud mode (`QUARK_MULTI_TENANT=1`): FORCE RLS enforced; every tenant-owned query runs in a tenant-tx.
- `api_tokens` and `sessions` do NOT get `FORCE` (by-hash lookup needs to find the row without `app.tenant_id` set); documented in code.
- No `CREATE INDEX CONCURRENTLY` (deadlocks under the boot advisory lock — proven in P1a).
- `src/codec.rs` / `src/permute.rs` untouched.
- Postgres tests gated behind `QUARK_TEST_DATABASE_URL`. Rust tests run `CARGO_BUILD_JOBS=1 cargo test -j1`.
- **Controller runs before merge:** the gated arm against real Postgres with `QUARK_MULTI_TENANT=1` (FORCE only exercises with a live DB) + a prod-dump dry-run of the FORCE migration.
- Out of scope: create tenant / signup / switcher (P2b); invites (P2c); OIDC per tenant (P2d); Host→tenant (P3).

## File Structure

- **Modify** `src/main.rs` — read `QUARK_MULTI_TENANT`; pass to store open + `AppState`.
- **Modify** `src/store/mod.rs` — `open_backends` accepts/propagates `multi_tenant`; `OutboxDelivery` gains `tenant_id`.
- **Modify** `src/store/postgres.rs` — `multi_tenant` field; `open*` set it; `begin_tenant_tx_read`; `with_read`/`with_write`; route all tenant-owned methods; `init_schema` conditional FORCE; `claim_due_deliveries` selects `tenant_id`.
- **Modify** `src/store/lmdb.rs` — `OutboxDelivery` construction (if any) carries `tenant_id` (LMDB outbox is a no-op, but the struct field must be set).
- **Modify** `src/api.rs` — `AppState.multi_tenant`; `create_link_core`/`resolve_code`/`resolve_for_admin` gain a `tenant` param; sheets OAuth state carries tenant.
- **Modify** `src/webhooks/delivery.rs` — relay resolves the subscription per delivery via `(tenant_id, subscription_id)`.
- **Test** `tests/tenant_enforcement.rs` (new, gated) + extend `tests/tenant_isolation.rs`.

---

### Task 1: Mode flag `QUARK_MULTI_TENANT`

**Files:**
- Modify: `src/main.rs` (read env, thread to store + AppState)
- Modify: `src/store/mod.rs` (`open_backends` signature), `src/store/postgres.rs` (`multi_tenant` field + constructors)
- Modify: `src/api.rs` (`AppState.multi_tenant: bool`)
- Test: inline `#[cfg(test)]` in `src/store/postgres.rs`

**Interfaces:**
- Produces: `AppState.multi_tenant: bool`; `PostgresStore.multi_tenant: bool`; `PostgresStore::open`/`open_with_replica` take a `multi_tenant: bool` argument; `open_backends(path, multi_tenant)`.

- [ ] **Step 1: Write the failing test** (the flag reaches the store)

```rust
// in src/store/postgres.rs #[cfg(test)] — pure constructor check, no DB needed
#[test]
fn multi_tenant_flag_defaults_false_and_is_settable() {
    // A lightweight check that the field exists and round-trips via the ctor param.
    // (No DB connection; assert the struct carries the flag.)
    // If a full ctor needs a DB, instead assert via a small helper `fn is_multi_tenant(&self)->bool`.
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_BUILD_JOBS=1 cargo test -j1 --lib store::postgres`
Expected: FAIL — field/param does not exist.

- [ ] **Step 3: Write minimal implementation**

`src/store/postgres.rs`: add `multi_tenant: bool` to the `PostgresStore` struct; `open`/`open_with_replica` take `multi_tenant: bool` and set it; add `pub fn is_multi_tenant(&self) -> bool { self.multi_tenant }`.

`src/store/mod.rs`: `open_backends(path: &Path, multi_tenant: bool)` threads it to `PostgresStore::open*` (LMDB ignores it — single-tenant only; store the flag but LMDB never forces anything).

`src/main.rs`: near the other env reads:
```rust
let multi_tenant = std::env::var("QUARK_MULTI_TENANT").map(|v| v != "0").unwrap_or(false);
if multi_tenant {
    eprintln!("multi-tenant mode ENABLED (FORCE RLS + per-tenant tx on Postgres)");
}
```
Pass `multi_tenant` to `open_backends(...)` and set `AppState { multi_tenant, .. }`.

`src/api.rs`: add `pub multi_tenant: bool` to `AppState` (update every `AppState { .. }` construction site — grep `AppState {` in `src/`, `tests/`, `benches/` — set `multi_tenant: false` in tests/benches).

- [ ] **Step 4: Run test to verify it passes**

Run: `CARGO_BUILD_JOBS=1 cargo test -j1 --lib store::postgres` then `CARGO_BUILD_JOBS=1 cargo build -j1`
Expected: PASS + whole tree compiles.

- [ ] **Step 5: Commit**

```bash
git add -u && git commit -m "feat(store): QUARK_MULTI_TENANT mode flag threaded to AppState + PostgresStore"
```

---

### Task 2: Tenant-tx helpers + FORCE RLS + route all tenant-owned Postgres methods

**Files:**
- Modify: `src/store/postgres.rs` (`begin_tenant_tx_read`; `with_read`/`with_write`; route methods; `init_schema` FORCE)
- Test: `tests/tenant_enforcement.rs` (new, gated)

**Interfaces:**
- Consumes: `multi_tenant` (Task 1), existing `begin_tenant_tx` (postgres.rs:578).
- Produces: every tenant-owned `PostgresStore` method enforces `app.tenant_id` in cloud mode; FORCE RLS on 11 tables.

- [ ] **Step 1: Write the failing test** (fail-closed: cloud + FORCE, no tenant set → 0 rows; correct tenant → rows)

```rust
// tests/tenant_enforcement.rs
#[tokio::test]
async fn cloud_force_rls_is_fail_closed() {
    let Some(url) = std::env::var("QUARK_TEST_DATABASE_URL").ok() else { return; };
    // multi_tenant = true
    let store = quark::store::PostgresStore::open(&url, true).await.unwrap();
    store.reset_for_tests().await.unwrap();
    let a = std::sync::Arc::new(store) as std::sync::Arc<dyn quark::store::Store>;
    let t1 = a.clone().for_tenant(quark::tenant::TenantId(1));
    let t2 = a.clone().for_tenant(quark::tenant::TenantId(2));
    let rec = /* build Record */;
    t1.put_link(700, &rec).await.unwrap();
    // Enforced by RLS (tenant-tx sets app.tenant_id), not just the WHERE:
    assert!(t1.get_link(700).await.unwrap().is_some());
    assert!(t2.get_link(700).await.unwrap().is_none());
    assert_eq!(t2.list_links(None, 100, None, None).await.unwrap().len(), 0);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `QUARK_TEST_DATABASE_URL=... CARGO_BUILD_JOBS=1 cargo test -j1 --test tenant_enforcement`
Expected: FAIL (before FORCE/tenant-tx, or a compile error for the helpers).

- [ ] **Step 3: Write minimal implementation**

Add the read-pool tenant-tx (mirror of `begin_tenant_tx`) and remove the `#[allow(dead_code)]` from `begin_tenant_tx`:

```rust
async fn begin_tenant_tx_read(&self, tenant: TenantId)
    -> Result<sqlx::Transaction<'_, sqlx::Postgres>, StoreError> {
    let mut tx = self.read.begin().await.map_err(StoreError::backend)?;
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(tenant.0.to_string()).execute(&mut *tx).await.map_err(StoreError::backend)?;
    Ok(tx)
}
```

Add two routing helpers. In OSS mode they hand out a pooled connection (no tx); in cloud mode a tenant-tx (committed after). Use a `PgConnection`-typed closure so both branches share one body:

```rust
use sqlx::PgConnection;
use futures_util::future::BoxFuture;

async fn with_read<T>(&self, tenant: TenantId,
    f: impl for<'c> FnOnce(&'c mut PgConnection) -> BoxFuture<'c, Result<T, sqlx::Error>>)
    -> Result<T, StoreError> {
    if self.multi_tenant {
        let mut tx = self.begin_tenant_tx_read(tenant).await?;
        let r = f(&mut tx).await.map_err(StoreError::backend)?;
        tx.commit().await.map_err(StoreError::backend)?;
        Ok(r)
    } else {
        let mut c = self.read.acquire().await.map_err(StoreError::backend)?;
        f(&mut c).await.map_err(StoreError::backend)
    }
}
// with_write: identical but `self.begin_tenant_tx(tenant)` / `self.write.acquire()`.
```

Route every tenant-owned method through `with_read` (reads/lists/aggregates) or `with_write` (insert/update/delete/tx). Example transformation:

```rust
// before
async fn get_link(&self, tenant: TenantId, id: u64) -> Result<Option<Record>, StoreError> {
    let row = sqlx::query_scalar::<_, serde_json::Value>("SELECT record FROM links WHERE tenant_id=$1 AND id=$2")
        .bind(tenant.0 as i64).bind(id as i64).fetch_optional(&self.read).await?;
    // ...
}
// after
async fn get_link(&self, tenant: TenantId, id: u64) -> Result<Option<Record>, StoreError> {
    let row = self.with_read(tenant, |c| Box::pin(async move {
        sqlx::query_scalar::<_, serde_json::Value>("SELECT record FROM links WHERE tenant_id=$1 AND id=$2")
            .bind(tenant.0 as i64).bind(id as i64).fetch_optional(&mut *c).await
    })).await?;
    // ...deserialize as today...
}
```

Apply to ALL tenant-owned methods (the tenant-owned list from P1a — links/aliases/webhooks/api_tokens/pixels/wellknown/health/visits/sheets_connection/tags/folders/sessions writes/etc.), keeping the existing `WHERE tenant_id` predicates (belt AND suspenders). Methods that already open their own transaction (`put_link_tx`, `put_alias_and_link_tx`, `delete_link_tx`) must, in cloud mode, run their existing tx as a tenant-tx (begin via `begin_tenant_tx`, keep the multi-statement body). The **global/infra** methods (`gc_sessions`, `try_acquire_*_lease`, `enqueue_deliveries`, `claim_due_deliveries`, `mark_*`) and the **by-hash** methods (`get_api_token_by_hash`, `get_session_by_hash`, `delete_session`) stay on the bare pool (NOT tenant-tx).

`init_schema` — after the existing `ENABLE ROW LEVEL SECURITY` + `CREATE POLICY` loop, add (cloud only):
```rust
if self.multi_tenant {
    // FORCE makes even the owner obey the policy. Excepted: api_tokens/sessions,
    // whose by-hash lookups must find the row before the tenant is known.
    for table in TENANT_OWNED_TABLES.iter().filter(|t| **t != "api_tokens" && **t != "sessions") {
        sqlx::query(&format!("ALTER TABLE {table} FORCE ROW LEVEL SECURITY"))
            .execute(&mut *conn).await.map_err(StoreError::backend)?;
    }
}
```
(`ALTER ... FORCE` is idempotent and metadata-only. Do NOT force api_tokens/sessions.)

- [ ] **Step 4: Run test to verify it passes**

Run: `QUARK_TEST_DATABASE_URL=... CARGO_BUILD_JOBS=1 cargo test -j1 --test tenant_enforcement` (needs a live PG; if unset the test early-returns and the controller runs it)
Expected: PASS. Also `CARGO_BUILD_JOBS=1 cargo test -j1` (OSS paths, non-gated) green.

- [ ] **Step 5: Commit**

```bash
git add -u && git commit -m "feat(store/pg): FORCE RLS + per-tenant tx routing (cloud mode), OSS unchanged"
```

---

### Task 3: Close the 4 tenant carry-overs

**Files:**
- Modify: `src/api.rs` (`create_link_core`, `resolve_code`, `resolve_for_admin` gain `tenant`; sheets OAuth state)
- Modify: `src/webhooks/delivery.rs` + `src/store/mod.rs` + `src/store/postgres.rs` (relay per-tenant)
- Test: `tests/` (carry-over assertions)

**Interfaces:**
- Consumes: `Principal.tenant` (P1b).
- Produces: `create_link_core(st, tenant, url, ...)`; `resolve_code(st, tenant, code)`; `resolve_for_admin(st, tenant, code)`; `OutboxDelivery.tenant_id`.

- [ ] **Step 1: Write the failing test**

```rust
// A test that admin_import writes under the principal's tenant, and that
// resolve_code/resolve_for_admin resolve within the passed tenant. Reuse the
// admin-gated harness in tests/api_it.rs. (In P2a all principals are tenant 0,
// so assert the tenant PARAM is threaded — e.g. a unit test that resolve_code
// with tenant T calls get_alias(T, code).)
```

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_BUILD_JOBS=1 cargo test -j1 <name>`
Expected: FAIL — functions don't take a tenant param.

- [ ] **Step 3: Write minimal implementation**

- `create_link_core` (`src/api.rs:363`): add `tenant: crate::tenant::TenantId` as the 2nd param (after `st`); replace the internal `DEFAULT_TENANT` uses (api.rs:~413,432,441,463) with `tenant`. Callers: `admin_import` passes `p.tenant`; the public `create` handler passes `crate::tenant::DEFAULT_TENANT` (Host→tenant is P3).
- `resolve_code` (`src/api.rs:771`): add `tenant` param; `get_alias(tenant, code)`. Callers: redirect/public pass `DEFAULT_TENANT`; the stats handler passes `p.tenant`.
- `resolve_for_admin` (`src/api.rs:2180`): add `tenant` param; use it in the alias branch. Callers `admin_link_delete`/`admin_link_patch` pass `p.tenant`.
- Sheets OAuth: in `sheets_connect`, include `p.tenant` in the signed state payload; in `sheets_callback`, read the tenant from the verified state and call `put_sheets_connection(tenant, ...)` (replaces `DEFAULT_TENANT` at api.rs:1738). Keep the state signature/verification otherwise unchanged.
- Webhook relay: add `tenant_id: TenantId` to `OutboxDelivery` (`src/store/mod.rs:220`); `claim_due_deliveries` (postgres.rs) selects `tenant_id` into it (`webhook_deliveries` already has the column from P1a); LMDB's no-op path sets `tenant_id: DEFAULT_TENANT`. In `src/webhooks/delivery.rs`, when delivering a claimed row, resolve its subscription via `store.get_webhook(delivery.tenant_id, delivery.subscription_id)` instead of a `DEFAULT_TENANT`-only snapshot; drop/replace `refresh_relay_snapshot`'s single-tenant `list_webhooks(DEFAULT_TENANT)` with the per-delivery lookup (or an all-tenant snapshot keyed by `(tenant, id)`).

- [ ] **Step 4: Run test to verify it passes**

Run: `CARGO_BUILD_JOBS=1 cargo test -j1` (full suite; gated relay/sheets PG tests run when `QUARK_TEST_DATABASE_URL` set)
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -u && git commit -m "feat(api,webhooks): thread real tenant through create_link_core/resolvers/sheets-oauth/relay"
```

---

### Task 4: Enforcement + OSS-parity test sweep

**Files:**
- Modify: `tests/tenant_enforcement.rs`, `tests/tenant_isolation.rs`
- Test: whole suite

**Interfaces:** Consumes everything above.

- [ ] **Step 1: Add the assertions**

```rust
// tenant_enforcement.rs (gated, multi_tenant=true):
// - fail-closed: raw query on a tenant-owned table with no app.tenant_id set → 0 rows.
// - a "forgot the WHERE" simulation still returns only the current tenant's rows.
// - hash lookups: put_api_token then get_api_token_by_hash works in cloud mode (table not FORCEd).
// OSS parity: the existing tenant_isolation suite + full suite pass with the flag OFF.
```

- [ ] **Step 2: Run — OSS parity (flag off)**

Run: `CARGO_BUILD_JOBS=1 cargo test -j1`
Expected: full pre-existing suite PASSES unchanged (Postgres-gated tests skip without `QUARK_TEST_DATABASE_URL`).

- [ ] **Step 3: Commit**

```bash
git add -u && git commit -m "test(tenant): FORCE RLS fail-closed + hash-lookup + OSS parity"
```

---

## Self-Review

**1. Spec coverage:** mode flag → Task 1; FORCE RLS + tenant-tx routing + api_tokens/sessions exception → Task 2; 4 carry-overs → Task 3; fail-closed/hash/OSS-parity tests → Tasks 2 & 4. ✓ All spec sections covered.

**2. Placeholder scan:** Test bodies in Tasks 1/3/4 describe assertions in prose because they hook the existing gated harness and the exact `Record`/`AppState` fixtures live in the repo — the implementer wires them to the real harness (flagged). All production-code steps carry concrete code (the `with_read` helper, the FORCE loop, the carry-over signatures). No TBD/"handle later".

**3. Type consistency:** `multi_tenant: bool` on both `AppState` and `PostgresStore`; `with_read`/`with_write` signatures consistent; `create_link_core(st, tenant, ...)` / `resolve_code(st, tenant, code)` / `resolve_for_admin(st, tenant, code)` and `OutboxDelivery.tenant_id: TenantId` used consistently across Tasks 1-3. The by-hash / global-infra exclusion list matches the P1b/P1a convention.
