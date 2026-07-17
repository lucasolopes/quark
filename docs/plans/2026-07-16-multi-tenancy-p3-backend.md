# Multi-tenancy P3-backend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Backend for custom domains per tenant (cloud-only): a `domains` table, `Host → tenant` resolution on the redirect hot path with cross-tenant isolation, per-domain alias namespace, self-serve domain CRUD with DNS (TXT) verification, wellknown-by-Host, and SSRF coverage of all registered hosts. Frontend is a separate plan.

**Architecture:** A new `domains` table (tenant-owned but in the `NOT_FORCED` RLS set, because the public redirect looks it up by Host before the tenant is known). A `HostRouter` (Moka L1 + optional L2 + breaker + pub/sub invalidation, copying the `Cache` pattern) maps `host → {domain_id, tenant_id}`. The redirect resolves the Host, then filters resolved records by tenant on custom domains (404 on mismatch); the shared host (`domain_id 0`) stays global. Aliases gain a `domain_id` column and a `(domain_id, alias)` PK. DNS TXT verification uses `hickory-resolver` (new dep), off the hot path.

**Tech Stack:** Rust 2021, axum + tokio, sqlx (Postgres), moka, `hickory-resolver` (new — TXT lookup). `src/codec.rs`/`src/permute.rs` are UNTOUCHABLE (numeric short-code namespace stays global).

## Global Constraints

- Code and identifiers in English. Comments/copy follow avoid-ai-writing (no em dashes, no AI-isms).
- **Cloud-only:** every new admin endpoint and the whole domains feature is gated on `st.multi_tenant` (return 404 in OSS, mirroring `src/api.rs:1765`/`:1820`). OSS (flag off) behavior is byte-for-byte unchanged and the current suite passes identically. A test asserts OSS parity.
- **`src/codec.rs`/`src/permute.rs` untouched.** Numeric codes stay global; isolation is enforced by a post-resolution tenant check, not by changing the code scheme.
- **No `CREATE INDEX CONCURRENTLY`** (init_schema runs every boot under an advisory lock; CONCURRENTLY deadlocks — established rule).
- Postgres gated by `QUARK_TEST_DATABASE_URL`; the mandatory verification runs against a real **NON-SUPERUSER** role in cloud mode (superusers bypass FORCE RLS). Gate `-j1` / `CARGO_BUILD_JOBS=1`.
- Tenant-owned store methods route through `with_read!`/`with_write!` (per-tenant tx in cloud, bare pool in OSS). The ONE exception is `get_domain_by_host` (public Host lookup) which runs on the bare pool — that is why `domains` is in `NOT_FORCED`.
- `hickory-resolver` is used ONLY in the on-demand verify path, never on the redirect hot path.

## Backend contract seams (from the code map)

- Redirect: `src/api.rs:1058` `redirect(...)`, `:983` `unlock(...)`; both call `resolve_code(&st, DEFAULT_TENANT, &code)` (`:773`) then `st.cache.get(id)`.
- Store trait: `src/store/mod.rs` — alias methods `get_alias` (`:303`), `put_alias_and_link`/`_tx` (`:304`/`:332`); wellknown (`:498-511`); `next_id`/sequences.
- Postgres: `src/store/postgres.rs` — `init_schema` (`:419`), `TENANT_OWNED_TABLES` (`:88`), `NOT_FORCED` (`:660`, currently 6 entries), `with_read!`/`with_write!` (`:22`/`:53`), `begin_tenant_tx`/`_read` (`:719`/`:736`), PK-rework DO-block template (`:532-579`), `aliases` table (`:440`, insert `:826`/`:865`).
- SSRF: `is_blocked_target` (`src/api.rs:735`), `is_internal_host` (`src/abuse/mod.rs:14`), `st.public_host` (`src/api.rs:42`).
- Cache pattern to copy: `src/cache/mod.rs` (Moka + `CacheTier` + `Breaker` + `Invalidator`); pub/sub invalidation `src/invalidate.rs`.
- Flag: `AppState.multi_tenant` (`src/api.rs:59`), `PostgresStore.multi_tenant` (`:358`), `QUARK_MULTI_TENANT` read `src/main.rs:69`.

## File Structure

- Create: `src/domain.rs` — `Domain`, `DomainStatus`, `DomainRoute` types.
- Create: `src/domain_router.rs` — `HostRouter` (host → route, cached).
- Modify: `src/store/mod.rs` — `Store` trait: domain methods + alias signature change.
- Modify: `src/store/postgres.rs` — `domains` table + migration, `aliases` rework, domain methods, `next_domain_id`.
- Modify: `src/store/lmdb.rs` (or the OSS backend) — alias signature change (OSS keeps `domain_id 0`), domain methods return "unsupported"/empty in OSS.
- Modify: `src/api.rs` — Host resolution + isolation in `redirect`/`unlock`; `create_link_core` `domain_id`; `/admin/domains` handlers; `serve_wellknown` by Host; `is_blocked_target` all-hosts.
- Modify: `src/main.rs` — build the `HostRouter`, wire it into `AppState`.
- Modify: `Cargo.toml` — add `hickory-resolver`.
- Tests: `tests/domains_it.rs` (new, PG-gated), plus additions to the redirect/alias/wellknown tests.

---

### Task 1: `domains` table + `Domain` type + store methods

**Files:**
- Create: `src/domain.rs`
- Modify: `src/store/mod.rs` (trait), `src/store/postgres.rs` (schema + impl), the OSS backend (`src/store/lmdb.rs`)
- Test: `tests/domains_it.rs`

**Interfaces:**
- Produces:
  - `src/domain.rs`: `pub struct Domain { pub id: u64, pub tenant_id: TenantId, pub host: String, pub token: String, pub status: DomainStatus, pub created: u64, pub verified_at: Option<u64> }`; `pub enum DomainStatus { Pending, Verified }` (serde as `"pending"`/`"verified"` via `#[serde(rename_all = "lowercase")]`); `pub struct DomainRoute { pub domain_id: u64, pub tenant_id: TenantId }`. `pub const SHARED_DOMAIN_ID: u64 = 0;`
  - `Store` trait methods: `async fn next_domain_id(&self) -> Result<u64, StoreError>`; `async fn get_domain_by_host(&self, host: &str) -> Result<Option<Domain>, StoreError>` (BARE pool, no tenant — public lookup); `async fn get_domain(&self, tenant: TenantId, id: u64) -> Result<Option<Domain>, StoreError>`; `async fn list_domains(&self, tenant: TenantId) -> Result<Vec<Domain>, StoreError>`; `async fn put_domain(&self, domain: &Domain) -> Result<(), StoreError>`; `async fn set_domain_status(&self, tenant: TenantId, id: u64, status: DomainStatus, verified_at: Option<u64>) -> Result<(), StoreError>`; `async fn delete_domain(&self, tenant: TenantId, id: u64) -> Result<(), StoreError>`.

- [ ] **Step 1: Write the failing isolation test**

In `tests/domains_it.rs` (gate on `QUARK_TEST_DATABASE_URL`, cloud mode; mirror `tests/workspace_it.rs` setup — non-superoser role, `multi_tenant = true`):

```rust
// tenant A creates domain go.acme.com; tenant B cannot see it via list_domains/get_domain,
// but get_domain_by_host("go.acme.com") (public, bare) returns it with tenant_id = A.
#[tokio::test]
async fn domains_are_tenant_isolated_but_host_lookup_is_public() {
    let Some(store) = pg_store_cloud().await else { return }; // skip if no DB
    let a = TenantId(/* create tenant A */);
    let b = TenantId(/* create tenant B */);
    let id = store.next_domain_id().await.unwrap();
    store.put_domain(&Domain { id, tenant_id: a, host: "go.acme.com".into(), token: "tok".into(), status: DomainStatus::Verified, created: 1, verified_at: Some(2) }).await.unwrap();
    assert_eq!(store.list_domains(a).await.unwrap().len(), 1);
    assert_eq!(store.list_domains(b).await.unwrap().len(), 0); // RLS isolates admin view
    assert!(store.get_domain(b, id).await.unwrap().is_none());  // B cannot fetch A's domain
    let byhost = store.get_domain_by_host("go.acme.com").await.unwrap().unwrap();
    assert_eq!(byhost.tenant_id, a);                            // public lookup crosses tenants by design
}
```

- [ ] **Step 2: Run it to confirm it fails** — `CARGO_BUILD_JOBS=1 cargo test --test domains_it -- --nocapture` → FAIL (types/methods missing).

- [ ] **Step 3: Add the `Domain` types** — create `src/domain.rs` with the types above; `mod domain;` + re-exports in `src/lib.rs` (follow how `tenant` is declared). Derive `Debug, Clone, PartialEq, Eq, Serialize, Deserialize` on `Domain`/`DomainRoute`; `DomainStatus` also `Copy`.

- [ ] **Step 4: Schema in `init_schema`** — in `src/store/postgres.rs`:
  - Add sequence: `"CREATE SEQUENCE IF NOT EXISTS quark_domain_id_seq START WITH 1"` (0 is the shared sentinel; alongside `:493`).
  - Add table (in the CREATE TABLE block):
    ```sql
    CREATE TABLE IF NOT EXISTS domains (
      id BIGINT PRIMARY KEY,
      tenant_id BIGINT NOT NULL DEFAULT 0,
      host TEXT NOT NULL UNIQUE,
      token TEXT NOT NULL,
      status TEXT NOT NULL,
      created BIGINT NOT NULL,
      verified_at BIGINT
    )
    ```
  - Add `"domains"` to `TENANT_OWNED_TABLES` (`:88`) so it gets RLS ENABLE + POLICY.
  - Add `"domains"` to `NOT_FORCED` (`:660`, bump `[&str; 6]` → `[&str; 7]`) with a comment: the public redirect looks it up by Host before the tenant is known, on the bare pool, so FORCE would fail that path closed.
  - Add a per-tenant index: `"CREATE INDEX IF NOT EXISTS domains_by_tenant_id ON domains (tenant_id)"` (near `:591`). (`host` already unique via the table.) NO CONCURRENTLY.

- [ ] **Step 5: Implement the store methods (Postgres)** — `next_domain_id` mirrors `next_tenant_id` (`nextval('quark_domain_id_seq')`). `get_domain_by_host` runs on the BARE read pool (`self.read`), `SELECT ... FROM domains WHERE host = $1` — no `with_read!`, no tenant tx. `get_domain`/`list_domains`/`set_domain_status`/`delete_domain`/`put_domain` go through `with_read!`/`with_write!` exactly like the wellknown methods, `WHERE tenant_id`-scoped (RLS enforces it in cloud). Map `DomainStatus` to/from the `status` TEXT.

- [ ] **Step 6: OSS backend** — in the LMDB/OSS backend: `next_domain_id`/`get_domain_by_host`/`list_domains` return empty/`None`/`StoreError::Unsupported` as appropriate (domains are cloud-only; OSS never calls the admin CRUD because the endpoints are gated). Keep it compiling; a one-line "unsupported in OSS" is fine. Add whatever `StoreError` variant is idiomatic (reuse an existing one if present).

- [ ] **Step 7: Run the test** — `CARGO_BUILD_JOBS=1 cargo test --test domains_it` (with DB) → PASS. Without DB it skips. Also `cargo build` + `cargo fmt`.

- [ ] **Step 8: Commit** — `git add -A && git commit -m "feat(store): domains table + Domain type + tenant-scoped CRUD (host lookup public)"`

---

### Task 2: aliases per-domain namespace

**Files:**
- Modify: `src/store/mod.rs` (alias method signatures), `src/store/postgres.rs` (schema rework + impl), OSS backend
- Test: `tests/domains_it.rs` (append)

**Interfaces:**
- Consumes: Task 1 types.
- Produces (signature change): `get_alias(&self, domain_id: u64, alias: &str) -> Result<Option<u64>, StoreError>`; `put_alias_and_link`/`put_alias_and_link_tx` gain a `domain_id: u64` parameter. Callers pass `SHARED_DOMAIN_ID` (0) unless a custom domain is chosen (Task 5 wires the real value).

- [ ] **Step 1: Write the failing test**

```rust
// Same alias in two different domains points to different links; domain 0 is the shared namespace.
#[tokio::test]
async fn alias_namespace_is_per_domain() {
    let Some(store) = pg_store_cloud().await else { return };
    // link 100 (tenant A), link 200 (tenant B) already inserted...
    store.put_alias_and_link(10 /*domain A*/, "promo", 100, &rec_a).await.unwrap();
    store.put_alias_and_link(20 /*domain B*/, "promo", 200, &rec_b).await.unwrap();
    assert_eq!(store.get_alias(10, "promo").await.unwrap(), Some(100));
    assert_eq!(store.get_alias(20, "promo").await.unwrap(), Some(200));
    assert_eq!(store.get_alias(0, "promo").await.unwrap(), None); // shared namespace untouched
}
```

- [ ] **Step 2: Run to confirm it fails** — FAIL (arity/PK).

- [ ] **Step 3: Schema rework (migration)** — in `init_schema`, after the `aliases` CREATE:
  - `"ALTER TABLE aliases ADD COLUMN IF NOT EXISTS domain_id BIGINT NOT NULL DEFAULT 0"` (existing rows → shared namespace 0).
  - PK rework via the `DO $$ ... $$` guard template (`:532-579`): if the current PK is `(alias)`, drop it and add PRIMARY KEY `(domain_id, alias)`. Use the same `pg_index`/`array_agg(a.attname::text ...)` guard shape proven in P1b (avoids the `name[] = text[]` cast bug). Idempotent.

- [ ] **Step 4: Update the impl** — `get_alias`: `WHERE domain_id = $1 AND alias = $2`. `put_alias_and_link_tx`: insert `(alias, id, tenant_id, domain_id)`, `ON CONFLICT (domain_id, alias) DO NOTHING`. Thread `domain_id` through the trait + both backends (OSS always 0).

- [ ] **Step 5: Fix all call sites** — every current caller of `get_alias`/`put_alias_and_link*` (redirect `resolve_code`, create/import, benches) passes `SHARED_DOMAIN_ID` for now (Task 4/5 replace with the real domain). `grep` for the method names; update each. Benches: `put_alias_and_link(0, ...)`.

- [ ] **Step 6: Run** — the new test PASS; full lib suite green; `cargo fmt`.

- [ ] **Step 7: Commit** — `git commit -m "feat(store): per-domain alias namespace ((domain_id, alias) PK; existing -> domain 0)"`

---

### Task 3: `HostRouter` (host → route, cached)

**Files:**
- Create: `src/domain_router.rs`
- Modify: `src/main.rs` (construct + wire into `AppState`), `src/api.rs` (`AppState` field)
- Test: unit tests in `src/domain_router.rs`

**Interfaces:**
- Consumes: `Store` (`get_domain_by_host`), Task 1 types, the cache/breaker/invalidator patterns.
- Produces: `pub struct HostRouter { ... }` with `pub fn new(store: Arc<dyn Store>, public_host: Option<String>, /* optional l2, invalidator */) -> Self`; `pub async fn resolve(&self, host: &str) -> Option<DomainRoute>`; `pub async fn invalidate(&self, host: &str)`. `resolve` returns `Some(DomainRoute{domain_id: 0, tenant_id: DEFAULT_TENANT})` for the shared `public_host`, `Some(route)` for a `Verified` custom host, `None` for unknown/pending (caller 404s).

- [ ] **Step 1: Write failing unit tests** — with a fake `Store` returning a canned domain: `resolve("go.acme.com")` → `Some({domain_id, tenant_id})` when Verified; `None` when Pending or absent; `resolve(public_host)` → shared route; a second `resolve` for the same host hits the L1 cache (Store called once); `invalidate(host)` drops it (Store called again after).

- [ ] **Step 2: Run to confirm fail.**

- [ ] **Step 3: Implement** — mirror `src/cache/mod.rs`: `moka::future::Cache<String, Option<DomainRoute>>` (cache negatives too, short TTL, so unknown hosts do not hammer the DB), TTL ~300s. Only `Verified` domains resolve to `Some(custom route)`; `Pending` caches as `None`. Wrap the Store call so an L2/DB stall cannot block (reuse the `Breaker` + `L2_OP_TIMEOUT` idea if an L2 tier is added; for v1 an L1-only Moka over the bare-pool `get_domain_by_host` is acceptable — document that L2 is a later add). Wire `Invalidator` (`src/invalidate.rs`) so add/remove/verify (Task 6) drops the mapping across replicas — if wiring pub/sub is heavy, expose `invalidate(host)` locally and note cross-replica invalidation piggybacks on the existing `Invalidator` channel in a follow-up; TTL bounds staleness regardless.

- [ ] **Step 4: Wire into `AppState`** — add `pub host_router: Arc<HostRouter>` to `AppState`; construct in `src/main.rs` (only meaningful in cloud; in OSS it resolves everything to the shared route). `public_host` comes from the existing `st.public_host` source.

- [ ] **Step 5: Run** — unit tests PASS; build + fmt.

- [ ] **Step 6: Commit** — `git commit -m "feat(router): HostRouter host->tenant route with cached lookup + invalidation"`

---

### Task 4: redirect/unlock Host resolution + cross-tenant isolation

**Files:**
- Modify: `src/api.rs` (`redirect`, `unlock`, `resolve_code`)
- Test: `tests/domains_it.rs` or an http-level test (append)

**Interfaces:**
- Consumes: `HostRouter` (Task 3), alias signature (Task 2).
- Produces: `resolve_code` takes `domain_id` (for the alias namespace); `redirect`/`unlock` resolve the Host first and apply the isolation filter.

- [ ] **Step 1: Write failing tests** — http-level (spawn the app, or unit on the resolution helper):
  - Numeric code of a link owned by tenant A: served on `go.acme.com` (A) → 302; served on `go.beta.com` (B) → 404.
  - Alias `promo` on domain A → link A; `promo` on domain B → link B (different targets).
  - Unknown host → 404. Shared host → resolves globally (existing behavior, a regression guard).

- [ ] **Step 2: Run to confirm fail.**

- [ ] **Step 3: Implement** — in `redirect`/`unlock`:
  1. Read `headers.get(header::HOST)` → normalize (lowercase, strip port). `let route = st.host_router.resolve(host).await;`
  2. `None` → return 404 immediately.
  3. `resolve_code(&st, route.domain_id, &code)` (numeric path unchanged/global; alias path uses `route.domain_id`).
  4. After `st.cache.get(id)` → `Record`: if `route.domain_id != SHARED_DOMAIN_ID` and `record.tenant_id != route.tenant_id.0` → 404 (isolation). Shared host: no filter.
  - Keep the existing analytics/unlock/password logic after the filter. Do not change the numeric decode.

- [ ] **Step 4: Run** — tests PASS (isolation + alias-per-domain + unknown-host 404 + shared global). Full suite green.

- [ ] **Step 5: Commit** — `git commit -m "feat(api): resolve Host->tenant in redirect + cross-tenant isolation filter"`

---

### Task 5: `create_link_core` domain selection

**Files:**
- Modify: `src/api.rs` (`create_link_core` and the create/import request types)
- Test: append

**Interfaces:**
- Consumes: Task 1 (`get_domain`), Task 2 (alias with `domain_id`).
- Produces: create accepts an optional `domain_id` (default `SHARED_DOMAIN_ID`); validates ownership+verified; the alias is stored in that domain's namespace.

- [ ] **Step 1: Write failing tests** — creating a link with `domain_id` of the tenant's own Verified domain succeeds and the alias resolves on that domain; `domain_id` of an unverified domain, or one owned by another tenant, → 4xx (validation), alias not created; default (no domain_id) → shared namespace (existing behavior).

- [ ] **Step 2: Run to confirm fail.**

- [ ] **Step 3: Implement** — add `domain_id: Option<u64>` to the create request; in `create_link_core`, if `Some(d)` and `d != 0`: `get_domain(principal.tenant, d)` must return a `Verified` domain owned by the caller (RLS already scopes to the tenant; also assert status). Reject otherwise (`400`/`404`). Pass the chosen `domain_id` into `put_alias_and_link`. `admin_import` defaults to shared (0) unless a per-row domain is specified (keep import shared-only for now; note it).

- [ ] **Step 4: Run** — tests PASS; suite green.

- [ ] **Step 5: Commit** — `git commit -m "feat(api): create_link_core picks alias domain namespace (verified+owned)"`

---

### Task 6: `/admin/domains` endpoints + DNS TXT verification

**Files:**
- Modify: `Cargo.toml` (`hickory-resolver`), `src/api.rs` (handlers + routes), a small `src/dns.rs` helper (TXT lookup)
- Test: append (mock the TXT lookup behind a trait/seam)

**Interfaces:**
- Consumes: Task 1 (domain CRUD), Task 3 (`host_router.invalidate`).
- Produces: `GET /admin/domains`, `POST /admin/domains {host}`, `POST /admin/domains/:id/verify`, `DELETE /admin/domains/:id` — all cloud-only, tenant-scoped via `admin_guard` `Principal`. A DNS seam `async fn lookup_txt(name: &str) -> Result<Vec<String>, DnsError>` (real impl via `hickory-resolver`) so tests can inject records.

- [ ] **Step 1: Write failing tests** — (a) create returns `pending` + the DNS instructions (TXT name `_quark-verify.<host>` = token, CNAME target); (b) verify with a mocked `lookup_txt` returning the token → status `verified` + `host_router` invalidated; (c) verify with wrong/absent TXT → stays `pending`; (d) list/delete are tenant-scoped (a second tenant does not see the first's domains — non-superuser PG); (e) all four return 404 in OSS (`multi_tenant=false`); (f) creating an internal host (`is_internal_host`) or `public_host` is rejected.

- [ ] **Step 2: Run to confirm fail.**

- [ ] **Step 3: Add the dep + DNS seam** — `Cargo.toml`: `hickory-resolver = { version = "*", default-features = false, features = ["tokio"] }` (pin to the current release; check the crate). `src/dns.rs`: a `Dns` trait with `lookup_txt`, a `HickoryDns` impl (system resolver), and the `AppState` holds `Arc<dyn Dns>` so tests inject a fake. Timeout the lookup (a few seconds); never on the hot path.

- [ ] **Step 4: Implement handlers** — gate `if !st.multi_tenant { return 404 }` first. Use `admin_guard` for the `Principal` (tenant). `POST` validates host format, rejects `is_internal_host(host)` and `host == public_host`, generates a token (reuse the token/secret generator used for API tokens/webhook secrets), `put_domain(pending)`, returns instructions. `verify` fetches the domain (tenant-scoped), `lookup_txt("_quark-verify.<host>")`, on token match `set_domain_status(Verified)` + `host_router.invalidate(host)`, rate-limited (`st.ratelimiter`). `DELETE` → `delete_domain` + `host_router.invalidate(host)`. Register routes near `:3349`.

- [ ] **Step 5: Run** — tests PASS (incl. non-superuser tenant scoping + OSS 404); build + fmt + clippy.

- [ ] **Step 6: Commit** — `git commit -m "feat(api): /admin/domains CRUD + DNS TXT verification (hickory-resolver)"`

---

### Task 7: wellknown-by-Host + SSRF all-hosts + OSS parity sweep

**Files:**
- Modify: `src/api.rs` (`serve_wellknown`, `is_blocked_target`)
- Test: append + OSS parity tests

**Interfaces:**
- Consumes: `HostRouter` (Task 3).
- Produces: `serve_wellknown` selects tenant by Host; `is_blocked_target` treats any resolvable quark host as self.

- [ ] **Step 1: Write failing tests** — (a) `serve_wellknown` on a Verified custom host serves that tenant's AASA; on the shared host serves tenant 0's; unknown host → 404. (b) creating a link whose target host is a Verified custom domain of quark → blocked (self-loop); an unrelated external host → allowed. (c) OSS parity: with `multi_tenant=false`, `serve_wellknown` behaves exactly as today (tenant 0, Host ignored), `/admin/domains` 404, redirect uses the single host, aliases global — the pre-P3 suite passes unchanged.

- [ ] **Step 2: Run to confirm fail.**

- [ ] **Step 3: Implement** — `serve_wellknown(st, name, headers)`: resolve Host via `host_router`; `None` → 404; else `get_wellknown(route.tenant_id, name)`. Shared host → `DEFAULT_TENANT` (unchanged). `is_blocked_target`: in addition to `st.public_host`, treat the target host as self if `st.host_router.resolve(target_host)` is `Some` (any registered quark host). Keep `is_internal_host` as-is. Guard the Host-aware branches behind `st.multi_tenant` so OSS is untouched.

- [ ] **Step 4: Run** — all PASS; full suite green; `cargo fmt` + `clippy`.

- [ ] **Step 5: Commit** — `git commit -m "feat(api): wellknown-by-Host + SSRF covers all registered hosts; OSS parity held"`

---

## Self-Review

**Spec coverage:** `domains` table + migration (T1); `Host → tenant` resolution + cache (T3) + hot-path wiring + isolation (T4); TXT+CNAME verification (T6); per-domain alias uniqueness (T2, T5); wellknown-by-Host (T7); SSRF all-hosts (T7); create-flow domain selection (T5); cloud-only gating + OSS parity (T6, T7). TLS is out of scope (documented). Frontend is the separate P3-frontend plan.

**Placeholder scan:** the L2/pub-sub wiring in T3 is intentionally staged (L1 + TTL for v1, cross-replica invalidation as a noted follow-up) rather than vague — the invariant (TTL bounds staleness, unknown/pending → 404) is explicit. `hickory-resolver` version is "check the crate/pin current" — the implementer pins the real current release.

**Type consistency:** `DomainRoute { domain_id: u64, tenant_id: TenantId }`, `SHARED_DOMAIN_ID = 0`, and the `get_alias(domain_id, alias)` / `put_alias_and_link(domain_id, ...)` signatures are used identically across T2/T4/T5. `get_domain_by_host` (bare) vs `get_domain`/`list_domains` (tenant-scoped) distinction is consistent T1→T6.

**Scope:** large but single-subsystem (domains) — kept as one backend plan; frontend split out. Each task ends with an independently testable deliverable and its own commit.

## Verification (whole-plan)

- PG-gated, NON-SUPERUSER, cloud: isolation (A code on B host → 404), alias-per-domain, domain CRUD tenant-scoping, verify with mocked TXT, wellknown-by-Host, SSRF self-loop.
- Migration dry-run over a prod dump: aliases → `domain_id 0` (global uniqueness preserved), `domains` created empty, RLS on, no `CONCURRENTLY`.
- OSS parity: flag off → endpoints 404, redirect/aliases/wellknown identical to pre-P3; existing suite green.
- `cargo build` / `cargo clippy --all-targets -- -D warnings` / `cargo fmt --check` clean; `-j1`.
