# Multi-tenancy P2c-backend (invites) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Backend for team invites (cloud-only): an `invites` table, Owner/Admin create/list/revoke, and a token-based accept flow that grants membership with the invited role — email-bound, single-use, expiring.

**Architecture:** New `invites` table (in `TENANT_OWNED_TABLES` + `NOT_FORCED`, because accept looks it up by token hash before the tenant is known, mirroring `api_tokens`). Token = `generate_token()` plaintext returned once, only `hash_token()` stored. Create/list/revoke are tenant-scoped via `admin_guard(Scope::Full)` (Owner/Admin). Accept uses `session_user_id` (a non-member must reach it), validates email binding + single-use + expiry, then `put_membership` + `set_session_tenant`.

**Tech Stack:** Rust 2021, axum, sqlx (Postgres), the existing `auth::{generate_token, hash_token}`, `st.ratelimiter`.

## Global Constraints

- English identifiers; comments follow avoid-ai-writing (no em dashes, no AI-isms).
- **Cloud-only:** every endpoint gated `if !st.multi_tenant { 404 }` (mirror `admin_tenants_create` `src/api.rs:1842`). OSS byte-for-byte unchanged; a test asserts OSS 404.
- Token plaintext NEVER persisted (only SHA-256 via `hash_token`). Accept never trusts client-supplied tenant/role — only the invite row.
- No `CREATE INDEX CONCURRENTLY`. Postgres gated by `QUARK_TEST_DATABASE_URL`, verified as a NON-SUPERUSER role in cloud mode (final gate). `-j1`.
- `src/codec.rs`/`src/permute.rs` untouched.
- Role invitable set is exactly {Admin, Member, Viewer} — never Owner.

## Backend seams (from the map)

- `src/tenant.rs`: `Membership`, `Role`, `role_scopes`. `src/store/mod.rs`: `put_membership`/`get_membership` (bare pool). `src/store/postgres.rs`: `TENANT_OWNED_TABLES` (~89), `NOT_FORCED` (~749), `api_tokens` table+`token_hash` idx (~500), `get_api_token_by_hash` (~1247), sequences, `reset_for_tests` (~786), `with_read!`/`with_write!` (~23/~54).
- `src/auth.rs`: `generate_token` (~67), `hash_token` (~80).
- `src/api.rs`: `admin_tenants_create` (~1837), `session_user_id` (~1787), `set_session_tenant` (~1805), `admin_guard`/`Principal` (~1362/~1384), `conflict_or_503`, route list (~3610), `admin_tokens_create` (~3336) as the token-returning-once precedent. A user's email: fetch via the store user lookup (find how `User` is fetched by id — grep `get_user`).

## File Structure

- `src/invite.rs` (or add to `src/tenant.rs`) — `Invite` + `InviteError` types.
- `src/store/mod.rs` / `src/store/postgres.rs` / OSS backend — `invites` table + store methods.
- `src/api.rs` — 4 handlers + routes.
- `tests/invites_it.rs` (new, PG-gated).

---

### Task 1: `invites` table + `Invite` type + store methods

**Files:** Create `src/invite.rs`; Modify `src/store/mod.rs`, `src/store/postgres.rs`, the OSS backend; Test `tests/invites_it.rs`.

**Interfaces produced:**
- `pub struct Invite { pub id: u64, pub tenant_id: TenantId, pub email: String, pub role: Role, pub token_hash: String, pub invited_by: u64, pub created: u64, pub expires: u64, pub accepted_at: Option<u64>, pub accepted_by: Option<u64> }`
- `Store`: `async fn next_invite_id(&self) -> Result<u64, StoreError>`; `async fn create_invite(&self, inv: &Invite) -> Result<(), StoreError>`; `async fn get_invite_by_hash(&self, token_hash: &str, now: u64) -> Result<Option<Invite>, StoreError>` (BARE pool; returns only rows with `accepted_at IS NULL AND expires >= now`); `async fn mark_invite_accepted(&self, id: u64, accepted_by: u64, now: u64) -> Result<(), StoreError>`; `async fn list_invites(&self, tenant: TenantId) -> Result<Vec<Invite>, StoreError>` (tenant-scoped, pending only); `async fn delete_invite(&self, tenant: TenantId, id: u64) -> Result<(), StoreError>`.

**Steps:**
- [ ] Write a failing PG-gated test in `tests/invites_it.rs` (mirror `tests/domains_it.rs` cloud/non-superuser harness): create an invite for tenant A; `get_invite_by_hash(hash, now)` returns it; after `mark_invite_accepted` it returns `None` (accepted); an invite with `expires < now` returns `None`; `list_invites(B)` does not see A's invite; `list_invites(A)` sees it.
- [ ] Run it, confirm it fails.
- [ ] `src/invite.rs`: define `Invite` (derive Debug/Clone/PartialEq/Eq/Serialize/Deserialize). `mod invite;` + re-export like `tenant`. `Role` is already in `tenant.rs`.
- [ ] `init_schema` (`src/store/postgres.rs`): add `CREATE SEQUENCE IF NOT EXISTS quark_invite_id_seq START WITH 1`; `CREATE TABLE IF NOT EXISTS invites (id BIGINT PRIMARY KEY, tenant_id BIGINT NOT NULL DEFAULT 0, email TEXT NOT NULL, role TEXT NOT NULL, token_hash TEXT NOT NULL, invited_by BIGINT NOT NULL, created BIGINT NOT NULL, expires BIGINT NOT NULL, accepted_at BIGINT, accepted_by BIGINT)`; `CREATE INDEX IF NOT EXISTS invites_token_hash_idx ON invites (token_hash)`; add `"invites"` to `TENANT_OWNED_TABLES` AND to `NOT_FORCED` (bump the array lengths; comment: accept looks it up by token hash before the tenant is known, on the bare pool). Add `invites` to `reset_for_tests` (TRUNCATE list + `ALTER SEQUENCE quark_invite_id_seq RESTART WITH 1`). NO CONCURRENTLY.
- [ ] Implement the store methods: `next_invite_id` mirrors `next_tenant_id`. `create_invite`/`get_invite_by_hash`/`mark_invite_accepted` run on the BARE pool (`self.write`/`self.read`) — no `with_*` (accept path is tenant-agnostic; `get_invite_by_hash` filters `accepted_at IS NULL AND expires >= $2`). `list_invites`/`delete_invite` go through `with_read!`/`with_write!` (tenant-scoped). Map `Role` ↔ TEXT (reuse the existing Role<->text mapping used by memberships/`domains.status` — grep how `role` is stored in `memberships`).
- [ ] OSS backend: cloud-only stubs (empty/None/`StoreError::Unsupported`), compiles.
- [ ] Run the test (PG) → pass; `cargo build`, `cargo fmt`, `cargo test --lib`.
- [ ] Commit `feat(store): invites table + Invite type + store methods`.

---

### Task 2: create / list / revoke endpoints

**Files:** Modify `src/api.rs`; Test append to `tests/invites_it.rs`.

**Interfaces produced:** `POST /admin/invites {email, role}` → 200 `{url, token, email, role, expires}`; `GET /admin/invites` → 200 `[{id, email, role, expires, created}]`; `DELETE /admin/invites/:id` → 200/404.

**Steps:**
- [ ] Write failing tests: create with role `Member` returns a token + link and stores only the hash; create with role `Owner` → 400; create as a `Viewer`/`Member` caller → 403 (admin_guard Scope::Full); list returns pending invites for the caller's tenant only; delete removes one; all three → 404 when `!multi_tenant`.
- [ ] Run, confirm fail.
- [ ] Implement:
  - `admin_invites_create`: `if !st.multi_tenant {404}`; `let p = admin_guard(&st, &headers, Scope::Full)?` (Owner/Admin); parse `role` (reject `Owner` → 400 with a clear message); rate-limit (`st.ratelimiter.check(&ip, now())`); normalize email lowercase; `let token = generate_token(); let id = next_invite_id()`; `create_invite(Invite { tenant_id: p.tenant, email, role, token_hash: hash_token(&token), invited_by: p.user_id, created: now(), expires: now()+7d, accepted_at: None, accepted_by: None })`; return `{ url: format!("{base}/invite/{token}", base = panel base or public), token, email, role, expires }`. (Use the panel/public base already used for other links, or return just the token and let the frontend build the URL — return both.)
  - `admin_invites_list`: guard + `list_invites(p.tenant)` → JSON without `token_hash`.
  - `admin_invites_delete`: guard + `delete_invite(p.tenant, id)`.
  - Register routes near `/admin/tenants` (`src/api.rs:~3640`). `7 * 24 * 3600` seconds = 7d expiry constant.
- [ ] Run tests → pass; build/fmt/lib.
- [ ] Commit `feat(api): /admin/invites create + list + revoke (Owner/Admin, cloud-only)`.

---

### Task 3: accept endpoint

**Files:** Modify `src/api.rs`; Test append.

**Interfaces produced:** `POST /admin/invites/:token/accept` → 200 (membership granted, session re-pointed) / 400 / 403 / 404 / 409 / 410.

**Steps:**
- [ ] Write failing tests (PG-gated, http-level): happy path — authenticated user whose `User.email` matches the invite email accepts → becomes a member with the invited role, invite marked accepted, session tenant = invite tenant; wrong token → 404; expired invite → 404/410; already-accepted (second accept) → 404/410; authenticated user whose email does NOT match → 403; user already a member of that tenant → 409; unauthenticated (no session) → 401.
- [ ] Run, confirm fail.
- [ ] Implement `admin_invites_accept`:
  - `if !st.multi_tenant {404}`; `let Some(user_id) = session_user_id(&st, &headers) else {401}`; rate-limit.
  - `let Some(inv) = get_invite_by_hash(&hash_token(token), now()) else { return NOT_FOUND }` (already filters accepted/expired).
  - Fetch the user's email (store user lookup by `user_id`); if `user.email.to_lowercase() != inv.email` → 403.
  - `if get_membership(user_id, inv.tenant_id).is_some()` → 409.
  - `put_membership(Membership { user_id, tenant_id: inv.tenant_id, role: inv.role, created: now() })`; `mark_invite_accepted(inv.id, user_id, now())`; `set_session_tenant(&st, &headers, inv.tenant_id)`; return 200 `{ tenant_id, role }`.
  - Order the checks so no membership is granted on any failure path; map store errors to 503.
- [ ] Run tests → pass; build/fmt/lib.
- [ ] Commit `feat(api): POST /admin/invites/:token/accept — email-bound, single-use, membership grant`.

---

### Task 4: OSS parity + security sweep (tests-only)

**Files:** Test append to `tests/invites_it.rs` (+ an ungated OSS test where possible).

**Steps:**
- [ ] Ungated (LMDB) OSS test: with `multi_tenant=false`, `POST/GET /admin/invites` and `POST /admin/invites/:token/accept` all return 404 without needing Postgres.
- [ ] Security sweep (PG-gated): single-use replay (accept twice → second 404/410, membership not duplicated/altered); token stored only as hash (query the row, assert `token_hash != token` and no plaintext column); create rejects `Owner` role; email binding negative; already-member 409; list/delete tenant-scoped (tenant B cannot see/delete A's invite) as a non-superuser.
- [ ] Run full suite; build/fmt/lib green.
- [ ] Commit `test(invites): OSS parity + security sweep (single-use, hash-at-rest, email-bind, tenant-scope)`.

## Self-Review

- Spec coverage: table+store (T1), create/list/revoke (T2), accept with all security checks (T3), OSS parity + security sweep (T4).
- Placeholder scan: the invite-link `base` in T2 — the implementer picks the existing panel/public base or returns the token for the frontend to build the URL (both returned); no TBD.
- Type consistency: `Invite` fields + `get_invite_by_hash(token_hash, now)` + `mark_invite_accepted(id, accepted_by, now)` used identically across T1/T3.

## Verification (whole-plan)

- PG-gated NON-SUPERUSER cloud: accept happy path + every security negative; tenant-scoped list/delete.
- OSS parity: flag off → all `/admin/invites` 404; existing suite green.
- `cargo build`/`clippy`/`fmt --check` clean; `-j1`. No `CONCURRENTLY`.
