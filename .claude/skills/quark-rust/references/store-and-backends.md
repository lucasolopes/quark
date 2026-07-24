# Store, backends, schema evolution, tenancy

## Trait shape

```rust
#[async_trait::async_trait]                    // full path, no `use async_trait::async_trait`
pub trait Store: Send + Sync + 'static {
    async fn get_link(&self, tenant: TenantId, id: u64) -> Result<Option<Record>, StoreError>;
}
```

- Data-plane traits (`Store`, `AnalyticsSink`, `CacheTier`) use the inline full
  path `#[async_trait::async_trait]`, repeated on every `impl`, and require
  `Send + Sync + 'static`. Evidence: `src/store/mod.rs:322-324`,
  `src/analytics/mod.rs:328-330`, `src/cache/mod.rs:33-38`.
  The newer HTTP/DNS seams (`Dns`, `KeycloakAdmin`, `SheetsApi`) use the imported
  form and omit `'static` - that is the minority; for a pluggable backend use the
  dominant form.
- Backends are injected as `Arc<dyn Trait>` fields on `AppState`; optional seams
  as `Option<Arc<dyn Trait>>` with a doc comment naming the env var that turns
  the feature on and what `None` means. **Never** parameterize handlers or
  `AppState` by generics: all wiring (`main`, `TestState`, `open_backends`)
  assumes trait objects.
- Errors: `StoreError` for store/analytics, `TierError` for cache. Convert
  library errors with `.map_err(StoreError::backend)`; local ones via
  `impl From` + `?`. Postgres UNIQUE violations go through
  `map_unique_violation` (SQLSTATE 23505 -> `StoreError::UniqueViolation`) so the
  handler can answer 409.
- Constructors: `pub async fn open(...) -> Result<Self, StoreError>`. Module
  helpers wrap in `Arc`; `open_postgres` returns the **concrete**
  `Arc<PostgresStore>` on purpose so tests can reach `reset_for_tests`, and the
  caller casts to `Arc<dyn Store>`.
- Register a new backend inside `store::open_backends` - that is the single seam.

## The trait stays total

Implement every method on **every** backend. When a backend cannot do it:

```rust
// src/store/lmdb.rs:1173-1180
// Cloud-only surface: never reached with the embedded backend.
// Kept as clear "unsupported" stubs instead of `unimplemented!()` so the trait
// stays total.
async fn put_domain(&self, ..) -> Result<(), StoreError> { Err(StoreError::Unsupported) }
```

Never `unimplemented!()` / `todo!()` / `panic!()`. `StoreError::Unsupported` maps
to 501. Returning a benign `Ok(None)` / `Ok(Vec::new())` is acceptable when the
caller treats absence as normal - but say so in a comment. The LMDB domains block
mixes both (writes error, reads degrade silently, because
`get_domain_by_host` is on the OSS redirect path); if you add methods there,
document which side you chose.

When only one backend has a capability, give the trait method a default body and
document who overrides it and why: `reencrypt_legacy_secrets` defaults to
`Ok(0)` (`src/store/mod.rs:824-835`); `click_totals` defaults to iterating
`stats()` and Postgres overrides it with one batched query
(`src/analytics/mod.rs:337-357`, `src/store/postgres.rs:3464-3489`).

Mutations that must emit a durable event get a pair: the normal method plus a
`*_tx` variant taking `deliveries: &[OutboxRow]` that writes everything in one
transaction. LMDB's `*_tx` delegates to the plain version (deliveries are always
empty there; events go through the in-memory channel).
Evidence: `src/store/mod.rs:342-380`, `src/store/lmdb.rs:1385-1401`.

## Tenancy

- `tenant: TenantId` is the **first** parameter of every tenant-owned method.
- Globally-unique hash/host lookups (token, session, invite, domain, sso domain)
  deliberately have no tenant parameter and **must** carry a doc comment saying
  the tenant travels in the value/row. Same for global infra methods
  (`next_*_id`, `list_tenants`, leases, outbox by `i64` id).
- Handlers pass `p.tenant` explicitly: `st.store.get_link(p.tenant, id)`.
  136 call sites do this. `ScopedStore` / `store.for_tenant(..)` exists
  (`src/store/mod.rs:874-890`) but is used **only** in tests - do not introduce
  it into `src/api`, it would create two competing styles.
- The only newtype id in the project is `pub struct TenantId(pub u64)` with
  sentinel `DEFAULT_TENANT = TenantId(0)`. Other ids (link id, domain id) are
  raw `u64`. Same-valued sentinels with different meaning get their own const and
  a doc explaining the distinction (`SHARED_DOMAIN_ID`).

### Postgres: RLS and the `with_read!` / `with_write!` macros

Never write `set_config('app.tenant_id', ...)` or open a transaction by hand in a
new `PostgresStore` method. Use the macros:

```rust
with_read!(self, tenant, |c| { /* query */ })
with_write!(self, tenant, |c| { /* mutation */ })
```

In cloud they open a transaction with `SET LOCAL` via `set_config(.., true)`
(transaction-scoped, so a pooled connection cannot leak the previous tenant) and
commit; in OSS they take a pool connection with no extra transaction,
byte-for-byte the pre-P2a path. `src/store/postgres.rs:35-100,1110-1159`.

The 18 tables in `TENANT_OWNED_TABLES` always get `ENABLE ROW LEVEL SECURITY`
plus a `<table>_tenant_isolation` policy (drop-then-create, since there is no
`CREATE POLICY IF NOT EXISTS`). `FORCE ROW LEVEL SECURITY` is applied **only** in
cloud, and never to the 11 tables in `NOT_FORCED` - those are the ones looked up
before the tenant is known (by hash: api_tokens/sessions/invites; by host:
domains/aliases/sso_email_domains/oidc_configs; cross-tenant: analytics, outbox).

Adding a table: if it has a pre-tenant lookup it goes in `NOT_FORCED` with a
comment justifying it; otherwise it is FORCED. The application-level
`WHERE tenant_id` predicate stays in **every** case - in OSS it is the only
isolation layer. Never drop it because "RLS covers it".

### LMDB: tenant prefix

Isolation is by key: `tkey(tenant, key)` / `tkey_id(tenant, id)` prefix
`tenant.0.to_be_bytes()` (8 bytes big-endian), giving a contiguous disjoint range
per tenant inside the same sub-db; scans use `tprefix(tenant)`. `meta` (global
counters) and `sessions` (global hash lookup) are deliberately unprefixed.
Aliases use `dkey(domain_id, ..)` because the alias namespace is per domain. The
prefix is ownership, never a partition of the code space (ids and short codes are
global).

Every cache hit is re-checked: `Cache::get` returns through
`owned_by(rec, tenant)`, which yields `None` when `rec.tenant_id != tenant`. L1/L2
stay keyed by bare `id`. Keep that choke point - a cache hit does not prove
ownership, and missing this check already caused a cross-tenant hit against real
Postgres (`src/cache/mod.rs:40-47,151-171`).

## Schema evolution: serde

Persisted types (`Record`, `ClickEvent`, `Aggregates`, `WebhookSubscription`,
`ApiToken`, `Session`, ...) are stored as serialized JSON in LMDB and in the
recent-events buffer. **Every new field gets `#[serde(default)]`**, and `Option`
fields get `#[serde(default, skip_serializing_if = "Option::is_none")]`. The doc
comment must state that old blobs need to deserialize rather than fail - the
project calls this load-bearing. In-memory-only PII fields use
`#[serde(skip)]`.

Add the regression test in the same file's `#[cfg(test)] mod`, deserializing the
literal old JSON and asserting the default (`src/store/mod.rs:1430-1440`).

Wire enums use per-variant `#[serde(rename = "...")]` plus hand-written
`as_str()` and `from_wire()` / `from_str_or_generic()` whose strings must match
the renames. Simple internal enums use `#[serde(rename_all = "lowercase")]`
(`RuleField`) or `"snake_case"` (`Role`). Unknown values fall back to the default
instead of erroring when the field has `#[serde(default)]`.

## Schema evolution: Postgres migrations

**There is no `migrations/` directory and no `sqlx::migrate!`.** Everything is an
array of idempotent DDL in `PostgresStore::init_schema`
(`src/store/postgres.rs:671-806`), called from `open()` at `:661` under
`SELECT pg_advisory_lock($1)` with `QUARK_SCHEMA_LOCK_ID` (concurrent
`CREATE TABLE IF NOT EXISTS` collides in the catalog).

- New column: append `"ALTER TABLE x ADD COLUMN IF NOT EXISTS y TYPE"` **to the
  end** of the array, with a comment naming the feature/phase that introduced it.
- New table: `CREATE TABLE IF NOT EXISTS` + an entry in `TENANT_OWNED_TABLES` if
  tenant-owned, which triggers the automatic `tenant_id BIGINT NOT NULL DEFAULT 0`
  backfill and the ENABLE RLS + policy drop/create.
- Primary key change: a `DO $$ ... $$` block that inspects the current PK columns
  via `pg_index`/`pg_attribute` before acting, so the index is not rebuilt every
  boot (`:835-898`).
- Indexes go in the array at `:916-929` **without** `CONCURRENTLY` - explicitly
  rejected because it deadlocks under the advisory lock and leaves the index
  INVALID (`:906-915`).
- Add the new table to the `reset_for_tests` TRUNCATE plus the
  `ALTER SEQUENCE ... RESTART` for its sequence (`:1088-1100`), otherwise gated
  tests leak state between cases.

## Schema evolution: LMDB sub-dbs

A new sub-db requires bumping `const MAX_DBS: u32 = 17;`
(`src/store/lmdb.rs:94`, whose comment counts the sub-dbs) - otherwise `open()`
fails at runtime, not compile time. `create_database` calls live in
`open_with_node_id` (`:161-207`). A tenant-owned sub-db also goes into
`const TENANT_OWNED_DBS: [&str; 12]` (`:70`), which includes it in the boot
re-keying migration `migrate_pre_tenancy_keys_to_default` (`:220-250`), guarded
by the `meta["tenancy_migrated"]` marker. Reserved mmap is
`MAP_SIZE_BYTES = 64 GiB`.

## Module layout

- `src/api/`: every submodule starts with `use super::*;`; shared imports are
  `pub(crate) use` at the top of `mod.rs`; `mod x;` + `pub(crate) use x::*;`
  (with `pub use` only for the public surface: `guard`, `links`, `router`,
  `tenants`).
- Backend submodules (`store/lmdb.rs`, `store/postgres.rs`, `cache/valkey.rs`,
  `analytics/clickhouse.rs`) do **not** use `use super::*` - they list
  `use crate::store::{...}` item by item. Reserve the glob for `src/api/`.
- `src/lib.rs` is a flat list of `pub mod` plus two shared helpers: `now()` and
  `hex()`. Reuse them instead of reimplementing epoch seconds or hex encoding.
