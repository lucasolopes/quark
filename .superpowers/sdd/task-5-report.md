# Task 5 Report: `record_pixel_health` no trait `Store`

## TDD evidence

### RED (Step 2)

```
export PATH="$HOME/.cargo/bin:$PATH" && cargo test -j1 --lib store::lmdb::tests::record_pixel_health
```

```
error[E0599]: no method named `record_pixel_health` found for struct `LmdbStore` in the current scope
    --> src\store\lmdb.rs:1926:11
help: there is a method `record_webhook_health` with a similar name
```

Test added in `src/store/lmdb.rs` (`record_pixel_health_updates_only_health_fields`), following the
same shape as the existing `record_webhook_health_updates_only_health_fields` test (uses
`LmdbStore::open_with_node_id` + `tempfile::tempdir`, not the brief's pseudo `new_test_store()`
helper, which does not exist in this codebase).

### GREEN (Step 7)

After implementing trait decl (Step 3), LMDB impl (Step 4), Postgres (Step 5), and fixing the two
stub sites (Step 6):

```
export PATH="$HOME/.cargo/bin:$PATH" && cargo test -j1 --lib store::lmdb::tests::record_pixel_health
```

```
running 1 test
test store::lmdb::tests::record_pixel_health_updates_only_health_fields ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 345 filtered out; finished in 0.02s
```

## Gated Postgres test

Added `record_pixel_health_updates_only_health_fields_pg` to `tests/pixel_store_it.rs`, mirroring
`record_webhook_health_updates_only_health_fields_pg` in `tests/webhooks_store_it.rs` (same
`fresh()` helper, same `#[file_serial]`, `eprintln!("skip: QUARK_TEST_DATABASE_URL not set")` early
return).

`QUARK_TEST_DATABASE_URL` is NOT set in this environment, so the test took its skip/early-return
path:

```
export PATH="$HOME/.cargo/bin:$PATH" && cargo test -j1 --test pixel_store_it record_pixel_health
running 1 test
test record_pixel_health_updates_only_health_fields_pg ... ok
```

(reported "ok" because the function returns `Ok(())` immediately on the `None` branch of `fresh()`
-- it did not touch a real database). The test compiles cleanly as part of `cargo build --all-targets`
and `cargo test`, confirming it will run for real once `QUARK_TEST_DATABASE_URL` is set.

## Every `impl Store for` site touched

- `src/store/mod.rs` -- trait declaration for `record_pixel_health` added next to the other pixel
  methods (after `list_pixels`).
- `src/store/lmdb.rs` (real impl, `LmdbStore`) -- `record_pixel_health` added right after
  `list_pixels`: surgical write-txn read-modify-write, no-op if the pixel key is absent.
- `src/store/postgres.rs` (real impl, `PostgresStore`) -- `record_pixel_health` added right after
  `put_pixel`: surgical `UPDATE pixels SET last_forward_at=$1, last_forward_status=$2 WHERE
  tenant_id=$3 AND id=$4`.
- `src/domain_router.rs` (`FakeStore` test stub) -- `record_pixel_health` added after `list_pixels`,
  body `Ok(())` (this stub already used `Ok(())` for `record_webhook_health` rather than
  `unimplemented!()`, so followed the same convention).
- `src/webhooks/delivery.rs` (`StubStore` test stub used by the webhook delivery worker tests) --
  `record_pixel_health` added after `list_pixels`, body `Ok(())`. This stub does capture
  `record_webhook_health` calls in a `Mutex<Vec<...>>` for webhook-delivery-worker assertions, but
  pixel forwarding is out of scope for this stub (that capture belongs to a later pixel-forwarding
  task, per the brief's "para a Task 7" note) -- a plain `Ok(())` is correct here since nothing in
  this file's tests exercises pixel health yet.

Confirmed via `grep -rn "impl Store for"` across `src/` that no other `impl Store for` blocks exist
besides these four. `cargo build --all-targets` initially reported exactly two missing-method
errors (`domain_router.rs`, `webhooks/delivery.rs`), and after the fix rebuilt clean.

## Postgres changes in detail (Step 5)

- 5a -- DDL (idempotent, inside the existing migrations block right after the `pixels` table
  creation): `ALTER TABLE pixels ADD COLUMN IF NOT EXISTS last_forward_at BIGINT` and
  `... last_forward_status JSONB`.
- 5b -- `row_to_pixel`: reads `last_forward_at` (`Option<i64>`) and `last_forward_status`
  (`Option<serde_json::Value>`), maps to `Option<u64>` / `HealthStatus` (defaulting to
  `HealthStatus::Never` when the column is `NULL`, e.g. rows written before this migration).
- 5c -- `get_pixel`'s and `list_pixels`'s `SELECT` column lists both extended with
  `last_forward_at, last_forward_status`.
- 5d -- `put_pixel`'s INSERT/ON CONFLICT extended with `last_forward_at`/`last_forward_status` as
  `$7`/`$8`, binding `config.last_forward_at.map(|v| v as i64)` and
  `serde_json::to_value(&config.last_forward_status)?` (hoisted to a `let` above `with_write!`,
  same pattern used for `credentials`, since `?` doesn't type-check as an inline expression inside
  the macro's closure).
- 5e -- `record_pixel_health` impl using `with_write!` + a single surgical `UPDATE`.

## `put_pixel` column/placeholder/bind alignment (explicit confirmation)

Checked line-by-line, all four lists in the same order, 8 items each:

- Columns: `id, provider, credentials, active, created, tenant_id, last_forward_at, last_forward_status`
- VALUES: `$1, $2, $3, $4, $5, $6, $7, $8`
- ON CONFLICT DO UPDATE SET: `provider=$2, credentials=$3, active=$4, created=$5, tenant_id=$6, last_forward_at=$7, last_forward_status=$8`
- `.bind(...)` chain: `config.id, provider_to_str(config.provider), &credentials, config.active, config.created, tenant.0, config.last_forward_at.map(...), &last_forward_status`

All four line up 1:1.

## Full gate (pristine output)

```
export PATH="$HOME/.cargo/bin:$PATH" && cargo fmt --all && cargo clippy -j1 --all-targets -- -D warnings && cargo test -j1 --lib
```

- `cargo fmt --all` -- no diff.
- `cargo clippy -j1 --all-targets -- -D warnings` -- `Finished` cleanly, zero warnings.
- `cargo test -j1 --lib` -- `test result: ok. 345 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out`.

## Self-review

- Trait method signature matches the brief exactly across all 4 sites (trait decl, LMDB, Postgres,
  and both stubs): `async fn record_pixel_health(&self, tenant: TenantId, id: u64, at: u64, status:
  crate::health::HealthStatus) -> Result<(), StoreError>`.
- LMDB impl is a true no-op (no write-txn commit) when the pixel key is absent -- matches the
  "no-op se o pixel nao existe mais" doc comment.
- Postgres `UPDATE` is unconditional (no existence check) but is a correct no-op in effect: 0 rows
  affected when the `(tenant_id, id)` pair doesn't exist, same externally-visible behavior as LMDB
  and consistent with `record_webhook_health`'s Postgres impl.
- Did not touch `reset_for_tests` TRUNCATE list, `src/codec.rs`, or `src/permute.rs`, per
  instructions.
- Did not add a new table.
- rustfmt-clean, clippy-clean, no new warnings introduced.
