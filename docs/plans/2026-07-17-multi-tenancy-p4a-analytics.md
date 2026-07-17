# Multi-tenancy P4a (analytics per-tenant) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Tag click events with `tenant_id` and enable per-tenant aggregate analytics, on the current Postgres backend, with the ClickHouse sink code prepped (validated at P4b when a server exists).

**Architecture:** `ClickEvent` gains `tenant_id` (stamped from the record at redirect); the Postgres analytics sink populates + queries it (the column already exists but is dead); a boot backfill fills existing rows from `links.tenant_id`; a new `/admin/stats` aggregates a tenant's links. ClickHouse sink gets the column/ORDER-BY/row/queries (compile-ready).

**Tech Stack:** Rust (axum, sqlx, the `clickhouse` crate). `src/codec.rs`/`src/permute.rs` UNTOUCHED.

## Global Constraints
- English; avoid-ai-writing. Cloud-aware; OSS = tenant 0 (aggregates everything, unchanged). Per-link `stats(id)` behavior preserved.
- No `CREATE INDEX CONCURRENTLY`. PG gated NON-SUPERUSER (`quark-postgres-1` local may be up: `postgres://quark:quark@localhost:5432/quark`). `-j1`.
- Analytics tables are NOT_FORCED → the tenant aggregate's isolation is 100% the app-level `WHERE tenant_id` predicate; it MUST be on every aggregate query.
- ClickHouse changes must compile + follow the existing `ALTER ... ADD COLUMN IF NOT EXISTS` idempotent pattern; real ClickHouse validation is deferred to P4b (no server in-process).

## Seams (from the map)
- `src/analytics/mod.rs`: `ClickEvent` (`:11-44`), `AnalyticsSink` trait (`:271-275`), `spawn_worker`/`flush` (`:304-387`), `Aggregates` (`:74-107`).
- `src/api.rs`: redirect `ClickEvent` construction (`:1304-1326`), `stats` handler (`:1366`), route registration.
- `src/store/postgres.rs`: `impl AnalyticsSink` `record_batch` (`:2515-2582`, 3 INSERTs `:2530-2569`), `stats` (`:2584-2678`), analytics DDL (`:599-602`), `TENANT_OWNED_TABLES` (`:91-108`, incl. the 3 analytics tables), the generic `ADD COLUMN tenant_id` loop (`:650-656`), `click_counters_by_tenant` idx (`:760`), `reset_for_tests`.
- `src/analytics/clickhouse.rs`: `ClickRow` (`:9-23`), DDL (`:104-106`), migration (`:113-137`), `record_batch` (`:152-180`), `stats` (`:182-355`).
- `src/store/mod.rs`: `open_backends` sink choice (`:938-977`).

---

### Task 1: `ClickEvent.tenant_id` + stamp + Postgres INSERTs + backfill

**Files:** `src/analytics/mod.rs`, `src/api.rs` (redirect), `src/store/postgres.rs` (INSERTs + boot backfill), `src/main.rs` (call the backfill) if needed; test.

**Steps:**
- [ ] Add `pub tenant_id: u64` to `ClickEvent` with `#[serde(default)]` (compat). Update all `ClickEvent { .. }` literals (redirect, tests, bench) to set it (0 where n/a).
- [ ] Redirect (`src/api.rs:1304-1326`): stamp `tenant_id: rec.tenant_id.0` (rec is authoritative). Confirm `rec` is in scope (it is, post-`cache.get`).
- [ ] Postgres `record_batch` (`postgres.rs:2530-2569`): bind `tenant_id` in the 3 INSERTs (`click_counters`, `stats_meta`, `click_events`) from `ev.tenant_id`. (The columns already exist.)
- [ ] Boot backfill (idempotent, cloud only or always-safe): in `init_schema` or a post-init step, run `UPDATE click_events ce SET tenant_id = l.tenant_id FROM links l WHERE l.id = ce.id AND ce.tenant_id = 0`, and the same for `click_counters`/`stats_meta`. Run under the advisory lock or tolerate races (idempotent — only touches `tenant_id=0` rows that have a matching link). NO CONCURRENTLY. Log a one-line count.
- [ ] Test (PG-gated): a click event recorded for a link owned by tenant B lands with `click_events.tenant_id = B` (query the row). Backfill: insert a click_events row with tenant_id=0 for a link owned by B, run the backfill, assert it becomes B; idempotent (run twice). LMDB/OSS: tenant 0, unaffected.
- [ ] Build/fmt/lib + gated test. Commit `feat(analytics): tag click events with tenant_id + backfill existing rows from links`.

---

### Task 2: `stats_for_tenant` + `GET /admin/stats` aggregate

**Files:** `src/analytics/mod.rs` (trait method), `src/store/postgres.rs` (impl), `src/store/lmdb.rs` (impl), `src/api.rs` (handler + route), test.

**Steps:**
- [ ] Add `async fn stats_for_tenant(&self, tenant: u64) -> Result<Aggregates, StoreError>` (or the sink's error type) to `AnalyticsSink`. (Aggregate only — no per-link `recent`.)
- [ ] Postgres impl: aggregate over `click_events`/`click_counters` `WHERE tenant_id = $1` (mirror `stats`'s aggregation but grouped for the tenant, not one id). LMDB impl: aggregate its in-memory/embedded analytics filtered by tenant (or return what it can — OSS is tenant 0). Update test-double sinks if any.
- [ ] `GET /admin/stats` handler: `admin_guard(&st, &headers, Scope::Analytics)` → `p.tenant` → `st.sink.stats_for_tenant(p.tenant)` → JSON `Aggregates`. Register the route near the other `/admin/*`.
- [ ] Tests (PG-gated): events for tenant A + tenant B; `stats_for_tenant(A)` counts only A's; `/admin/stats` as a Principal of B returns B's aggregate only (isolation — not A's). OSS: tenant 0 aggregates all.
- [ ] Build/fmt/lib + gated test. Commit `feat(api): GET /admin/stats — per-tenant aggregate analytics`.

---

### Task 3: ClickHouse sink tenant_id (compile-ready for P4b)

**Files:** `src/analytics/clickhouse.rs`; test (compile + unit where possible; real CH deferred).

**Steps:**
- [ ] `ClickRow` (`:9-23`): add `tenant_id: u64`. `record_batch` (`:163-175`): set `tenant_id: e.tenant_id`.
- [ ] DDL (`:104-106`): `ORDER BY (tenant_id, id, ts)` on create; add idempotent `ALTER TABLE clicks ADD COLUMN IF NOT EXISTS tenant_id UInt64 DEFAULT 0` in the migration block (`:113-137` pattern).
- [ ] Implement `stats_for_tenant(tenant)` for `ClickHouseSink` (`WHERE tenant_id = ?` aggregate) + add the tenant predicate is NOT required on per-link `stats(id)` (keep as-is). Ensure `AnalyticsSink` is fully implemented (the new trait method).
- [ ] Verify: `cargo build` compiles the clickhouse module; do NOT require a live ClickHouse (note real validation is P4b/LUC-9). If there are any pure-unit tests for `ClickRow` mapping, add one for the tenant_id field.
- [ ] Build/fmt/lib. Commit `feat(analytics): ClickHouse sink carries tenant_id (schema/row/stats) — validated at P4b`.

## Verification (whole-plan)
- PG-gated NON-SUPERUSER: events tagged with the owning tenant; backfill idempotent; `/admin/stats` isolated per tenant; per-link `stats` unchanged; OSS = tenant 0.
- ClickHouse module compiles with tenant_id; real validation deferred to P4b (provision + backfill job).
- `cargo build`/`clippy`/`fmt` clean; `-j1`. Then hand the user the ClickHouse provisioning runbook (P4b).
