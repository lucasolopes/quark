# Task 5 report — tenant-correct PKs (sheets_connection, wellknown_documents)

## Status: DONE

## Commit
143c024 — feat(store/pg): tenant-correct PKs for sheets_connection and wellknown_documents

## What changed

`src/store/postgres.rs`:
- Fresh-DB `CREATE TABLE IF NOT EXISTS sheets_connection` now defines
  `(tenant_id BIGINT NOT NULL DEFAULT 0 PRIMARY KEY, blob JSONB NOT NULL)`
  directly (no `singleton`).
- New idempotent migration block, placed **after** the `tenant_id` column
  backfill loop (both new PKs include `tenant_id`, so ordering matters):
  - `sheets_connection`: `DROP CONSTRAINT IF EXISTS sheets_connection_pkey`
    -> `DROP COLUMN IF EXISTS singleton` -> `DROP INDEX IF EXISTS
    sheets_connection_by_tenant` (now redundant with the PK) -> `DO $$ ...
    IF NOT EXISTS (pg_constraint) ... ADD PRIMARY KEY (tenant_id) ... $$`.
  - `wellknown_documents`: `DROP CONSTRAINT IF EXISTS
    wellknown_documents_pkey` -> guarded `ADD PRIMARY KEY (tenant_id, name)`.
  - All plain statements, no `CREATE INDEX CONCURRENTLY` (deadlocks under
    the boot advisory lock, per the P1a lesson already documented in the
    file).
- `put_sheets_connection`: `INSERT INTO sheets_connection (tenant_id, blob)
  VALUES ($1, $2) ON CONFLICT (tenant_id) DO UPDATE SET blob =
  EXCLUDED.blob` (no more `singleton` column, no more inserting it).
- `put_wellknown`: `ON CONFLICT (tenant_id, name) DO UPDATE SET body =
  EXCLUDED.body` (was `ON CONFLICT (name)`).

`tests/tenant_isolation.rs`: added gated test
`pg_wellknown_and_sheets_pks_are_tenant_correct` — two tenants put a
wellknown doc under the same name, both coexist and read back correctly;
one tenant upserts sheets_connection twice, reads back the latest, the
other tenant sees none; then `open_postgres` (runs `init_schema`) is called
twice more against the same DB to confirm the PK migration is a no-op on
re-run.

## Test summary

`QUARK_TEST_DATABASE_URL` was **not set** in this shell, so the gated PG
arm (including the new test) short-circuited via its early return rather
than exercising the real migration/upsert path against Postgres. Full
local run `CARGO_BUILD_JOBS=1 cargo test -j1` (every test binary): all
green, 0 failed, including `tenant_isolation` (9/9 ok, LMDB arm fully
exercised) and the pre-existing `sheets_connection_round_trips_pg` /
`wellknown_round_trip_pg` in `postgres_store_it.rs` / `store_it.rs` (ok as
no-op skips). Compile-correctness and the non-gated suite are confirmed
green; the gated PG arm still needs a live Postgres to actually prove the
migration and upsert behavior end to end — flagged for the controller's
gated-arm pass.

## Concerns

- Migration correctness against a real Postgres (idempotency re-run,
  actual PK behavior) is unverified in this session — no
  `QUARK_TEST_DATABASE_URL` available. Everything else (compile, non-gated
  suite, code review of the DDL ordering) checks out.
- Did not touch `codec.rs` / `permute.rs` (out of scope, untouched).
- LMDB unchanged, as specified (already tenant-keyed).
- Dropped `sheets_connection_by_tenant` index outright (the brief's
  preferred option) rather than leaving it as a harmless redundant index.

## Follow-up: review fixes (Important + Minor)

### Status: DONE

### What changed

`src/store/postgres.rs`, `init_schema`:

- **Important**: the PK-rework migration ran `DROP CONSTRAINT IF EXISTS
  <table>_pkey` unconditionally every boot, so the old `DO $$ IF NOT EXISTS
  (pg_constraint) ... ADD PRIMARY KEY $$` guard was always true — the PK
  index was dropped and rebuilt under ACCESS EXCLUSIVE on every restart,
  forever, not just once. Rewrote both `sheets_connection` and
  `wellknown_documents` migrations as single `DO $$ ... $$` blocks that
  check the *current* PK's column set against the target via
  `pg_index`/`pg_attribute`/`pg_class` (`array_agg(a.attname ORDER BY
  a.attname)` compared to the sorted target array) **before** doing
  anything, and only run `DROP CONSTRAINT` / `DROP COLUMN` / `ADD PRIMARY
  KEY` when the current PK does not already match. Once migrated, the block
  is now a true no-op — the `IF NOT EXISTS(...)` guard is false and nothing
  executes.
  - `sheets_connection` target: `ARRAY['tenant_id']`.
  - `wellknown_documents` target: `ARRAY['name', 'tenant_id']` (alphabetical
    order, matching `array_agg(... ORDER BY attname)`).
  - `DROP INDEX IF EXISTS sheets_connection_by_tenant` kept as its own
    statement (true no-op already — dropping a nonexistent index takes no
    lock and does no rebuild, so it wasn't part of this finding).
  - Fixed the stale comment above the block that claimed `DROP CONSTRAINT
    IF EXISTS ... is a no-op on a table that's already been migrated` —
    that was the inaccurate premise the bug hid behind. The comment now
    describes the actual mechanism (catalog check gates the whole block).
- **Minor**: fresh-DB `CREATE TABLE IF NOT EXISTS wellknown_documents` still
  declared `name TEXT PRIMARY KEY`. Changed to `(name TEXT NOT NULL, body
  TEXT NOT NULL, tenant_id BIGINT NOT NULL DEFAULT 0, PRIMARY KEY (tenant_id,
  name))` so a brand-new DB gets the target shape directly. Confirmed
  `sheets_connection`'s fresh `CREATE TABLE` already had `tenant_id ...
  PRIMARY KEY` from the original Task 5 change — left as is, its migration
  `DO` block follows the same skip-when-correct pattern as
  `wellknown_documents`.

Did not touch `codec.rs` / `permute.rs`. No test files changed (existing
`wellknown_round_trip_pg`, `sheets_connection_round_trips_pg`, and the
`pg_wellknown_and_sheets_pks_are_tenant_correct` no-op-reboot test already
cover both fresh-create and re-run-migration paths at the assertion level;
only the gated PG arm can actually prove the "no ALTER runs on the second
boot" behavior against a real catalog).

### Test command + output

```
cd quark
export PATH="$HOME/.cargo/bin:$PATH"
CARGO_BUILD_JOBS=1 cargo build -j1
CARGO_BUILD_JOBS=1 cargo test -j1
```

- `cargo build`: `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 35.44s` — no warnings, no errors.
- `cargo test`: every test binary green, including:
  - `postgres_store_it.rs`: 25 passed (0 failed) — `sheets_connection_round_trips_pg`, `wellknown_round_trip_pg` ok.
  - `tenant_isolation.rs`: 9 passed (0 failed) — `pg_wellknown_and_sheets_pks_are_tenant_correct`, `migration_seeds_default_tenant_and_columns` ok.
  - All other suites (`store_it`, `store_trait`, `webhook_outbox_it`, `webhooks_api_it`, `webhooks_store_it`, `tokens_api_it`, `valkey_tier_it`, `pubsub_invalidation_it`, `search_it`, unit tests): all ok.
- `QUARK_TEST_DATABASE_URL` was unset in this shell, so gated-PG assertions short-circuit via early return (as designed) rather than hitting real Postgres — compile + non-gated suite confirmed green; the controller's gated arm + prod-dump dry-run is the step that proves the no-op/idempotency behavior against a real catalog end to end.

### Concerns

- Same as before: real-Postgres verification of the no-op behavior (second
  boot performs zero ALTER/DROP CONSTRAINT statements against the catalog)
  is unverified in this session for lack of `QUARK_TEST_DATABASE_URL` —
  flagged for the controller's gated-arm + prod-dump dry-run pass.
- The catalog-check subquery assumes a single-column btree PK per table
  (true for both targets here); it does not generalize to composite indexes
  with expressions, but neither table needs that.
