# Multi-tenancy P3-completion (subdomains + Task 5) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Finish P3 — every cloud tenant auto-gets `<slug>.<suffix>` as a materialized Verified `domains` row (reuses all P3 machinery, no HostRouter/collision changes), the create-flow stamps the caller's real tenant + puts aliases in the tenant's default (subdomain) namespace, and the panel builds subdomain short URLs (fixes LUC-13). Cloud-only; OSS unchanged.

**Architecture:** A tenant's subdomain is just a normal Verified `domains` row (id from `next_domain_id()`) seeded at tenant-create + backfilled on boot; resolution/isolation/wellknown/SSRF work unchanged via `get_domain_by_host`/`owned_by`. Config `QUARK_TENANT_DOMAIN_SUFFIX` gates it. Task 5's core is fixing `create()` to use the resolved `Principal` (today it discards it and hardcodes `DEFAULT_TENANT`).

**Tech Stack:** Rust (axum/sqlx), React/TS. `src/codec.rs`/`src/permute.rs` UNTOUCHED.

## Global Constraints
- English; avoid-ai-writing. Cloud-only; OSS byte-for-byte unchanged (suffix unset OR flag off → no seed, create stamps tenant 0 / domain 0 as today). Tests assert OSS parity.
- No `CREATE INDEX CONCURRENTLY`. Postgres gated by `QUARK_TEST_DATABASE_URL`, verified NON-SUPERUSER in cloud (final gate). `-j1`.
- Subdomain rows materialized via `next_domain_id()` (same sequence as custom domains) — never reuse `tenant_id` as `domain_id`.
- Seeding + backfill idempotent (ON CONFLICT on `domains.host` UNIQUE).

## Seams (from the map)
- `admin_tenants_create` (`src/api.rs:~1837`); `AppState` (add the suffix config, read in `src/main.rs`).
- `next_domain_id`/`put_domain`/`get_domain_by_host` (`src/store/postgres.rs`); `domains` row shape (`src/domain.rs`).
- boot: `src/main.rs` after `open_backends`/`init_schema` — a backfill pass.
- `require_admin_for_create` (`src/api.rs:346`, discards Principal), `create()` (`:529`, hardcodes DEFAULT_TENANT at `:595`), `create_link_core` (`:378`, alias hardcodes `SHARED_DOMAIN_ID` at `:453`). `admin_import` (`:637`) is the correct precedent.
- `/admin/me` (`src/api.rs:1724`) — add the suffix to the response.
- `web/src/components/LinkTable.tsx:27-33` (`shortUrl`); `useMe()` (`web/src/lib/queries.ts:34`); `MeResponse` (`web/src/lib/types.ts`).

---

### Task 1: subdomain config + seed on create + boot backfill + expose suffix in /admin/me

**Files:** `src/api.rs` (AppState field, `admin_tenants_create`, `admin_me`), `src/main.rs` (read env + backfill), `src/store/mod.rs`/`postgres.rs`/`lmdb.rs` (a `list_tenants()` if needed for backfill), test `tests/domains_it.rs` or `tests/workspace_it.rs`.

**Steps:**
- [ ] Add `tenant_domain_suffix: Option<String>` to `AppState`; read `QUARK_TENANT_DOMAIN_SUFFIX` in `main.rs` (lowercased/trimmed), thread in. Update all `AppState` literal sites (tests/bench) — scripted insert + `cargo test --no-run`.
- [ ] Helper `subdomain_host(slug, suffix) -> String` = `format!("{}.{}", slug, suffix).to_ascii_lowercase()`.
- [ ] In `admin_tenants_create`, after `put_tenant` + membership: if `st.multi_tenant` and `Some(suffix)`, build the subdomain host and create a Verified `domains` row via `next_domain_id()` + `put_domain(Domain{status: Verified, host, tenant_id, token: String::new(), verified_at: Some(now), created: now, id})`. On `UniqueViolation` (already seeded) treat as success. Invalidate `host_router` for that host.
- [ ] Boot backfill in `main.rs` (cloud + suffix only): fetch all tenants (`list_tenants()` — add to Store if absent: `SELECT id, name, slug, created FROM tenants`), for each ensure a subdomain `domains` row exists (`get_domain_by_host` miss → create). Idempotent; ON CONFLICT on host. Log a one-line summary. (Runs once per boot, few tenants.)
- [ ] `admin_me`: add `"tenant_domain_suffix": st.tenant_domain_suffix` to the JSON (null when unset) so the frontend can build subdomain URLs.
- [ ] Tests (PG-gated cloud): creating a tenant seeds a Verified `domains` row `<slug>.<suffix>`; `get_domain_by_host` resolves it to the tenant; backfill seeds a pre-existing tenant; idempotent (run seed twice → one row); suffix unset → no seed. `/admin/me` includes the suffix.
- [ ] Build/fmt/lib + gated test (skip w/o DB). Commit `feat(api): auto per-tenant subdomain as a materialized verified domain + suffix in /admin/me`.

---

### Task 2: create-flow fix (Task 5) — stamp real tenant + alias in default domain

**Files:** `src/api.rs` (`require_admin_for_create`, `create`, `create_link_core`), test append.

**Steps:**
- [ ] Add helper `async fn default_domain_id(st, tenant) -> u64`: cloud + suffix → look up the tenant's subdomain `domains` row (`get_domain_by_host(subdomain_host(slug, suffix))`) and return its `id`; else `SHARED_DOMAIN_ID`. (Needs the tenant's slug — `get_tenant(tenant)` → slug.)
- [ ] Change `require_admin_for_create` (`src/api.rs:346`) to return `Result<Principal, StatusCode>` (stop discarding). Update its callers.
- [ ] In `create()` (`:595`): pass `p.tenant` (not `DEFAULT_TENANT`) to `create_link_core`, and pass `default_domain_id(st, p.tenant)` as the alias domain.
- [ ] `create_link_core`: add a `domain_id: u64` param (or compute from tenant inside) used at the `put_alias_and_link_tx` call (`:453`) instead of the hardcoded `SHARED_DOMAIN_ID`. Numeric code path unchanged (global).
- [ ] Tests: a cloud Principal creating a link → `Record.tenant_id == p.tenant` (not 0); its alias resolves on `<slug>.<suffix>/<alias>` (isolation: not on another tenant's subdomain); OSS create → tenant 0 + domain 0 (unchanged); `admin_import` still correct.
- [ ] Build/fmt/lib + gated test. Commit `feat(api): create stamps caller tenant + alias in tenant default (subdomain) namespace [Task 5]`.

---

### Task 3: frontend subdomain shortUrl (LUC-13)

**Files:** `web/src/lib/types.ts` (`MeResponse.tenant_domain_suffix`), `web/src/components/LinkTable.tsx` (`shortUrl`), tests.

**Steps:**
- [ ] Add `tenant_domain_suffix?: string | null` to `MeResponse`.
- [ ] In `LinkTable.tsx`, make `shortUrl(code)` tenant-aware: via `useMe()`, if cloud with a current membership slug + `tenant_domain_suffix`, build `https://<slug>.<suffix>/<code>`; else fall back to `PUBLIC_BASE` (OSS/no slug). Update the QR dialog + copy paths that use `shortUrl`. Update `LinkTable.test.tsx` (asserts the old `${origin}/code`).
- [ ] Tests: cloud with slug+suffix → subdomain URL copied/shown; OSS/no-suffix → `PUBLIC_BASE` fallback (regression guard).
- [ ] `npm run typecheck` + `npm run test` + `npm run lint` (independent). Commit `fix(web): subdomain-aware shortUrl (LUC-13)`.

## Verification (whole-plan)
- PG-gated NON-SUPERUSER cloud: seed+backfill idempotent, subdomain isolation (A code/alias on B subdomain → 404), create stamps real tenant.
- OSS parity: suffix unset/flag off → no seed, create identical, redirect identical.
- Frontend: subdomain URL in cloud, fallback in OSS.
- Infra prerequisite (documented, user-provisioned): `*.<suffix>` wildcard DNS + TLS + apex routing to the redirect app.
