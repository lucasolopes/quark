# End-to-end checklists

## New admin endpoint

Eight places change. Miss one and the endpoint either does not exist at runtime
or breaks CI.

1. **Request/response types** - serde structs inline in the owning
   `src/api/*.rs`: `#[derive(Deserialize)] pub(crate) struct XReq`,
   `#[derive(Serialize)] pub(crate) struct XResp`, with
   `#[serde(skip_serializing_if = "Option::is_none")]` on every `Option`.
   Query params get a `ListParams`-style struct.
2. **Handler** - `pub(crate) async fn admin_<resource>_<verb>` in the submodule.
   Order: multi-tenant gate -> `admin_guard` with the right `Scope` ->
   `csrf_guard` if it is a body-less mutation -> validation -> store call ->
   response. Returns `Response`.
3. **Module wiring** - if the file is new: `mod x;` + `pub(crate) use x::*;` in
   `src/api/mod.rs:117-141` (`pub use` only for the public surface).
4. **Route** - in the single chain in `src/api/router.rs:83-206`, **before**
   `.with_state(state)` at line 206. Param syntax is `:code`, not `{code}`.
   Verbs beyond get/post use the full path `axum::routing::delete(...)`.
5. **Store method** - if new, add to the `Store` trait and implement in **both**
   backends (`lmdb.rs` and `postgres.rs`); `Err(StoreError::Unsupported)` where
   not applicable. `TenantId` first.
6. **Integration test** - `tests/<area>_it.rs` with `mod common;` and
   `TestState::new(store, sink).build()`, exercised via `oneshot`. Assert the
   status for every error branch you wrote.
7. **Docs** - a `### \`METHOD /route\`` section in the right area of
   `docs/API.md` and the same in `docs/API.PT_BR.md`. Existing areas:
   Authentication, Public routes, Analytics, Link management, Webhooks,
   Conversion pixels, API tokens, Well-known documents, CORS.
8. **Panel**, if it consumes the endpoint - a method in `web/src/lib/api.ts`, the
   type in `web/src/lib/types.ts`, a hook + query key in `web/src/lib/queries.ts`,
   and any new UI string in **both** `web/src/i18n/en.ts` and
   `web/src/i18n/pt-BR.ts` (`en` exports the type, `ptBR: Messages` is what
   enforces parity at compile time - a missing key does not throw, it renders the
   raw key).

**Trap:** an endpoint whose "disabled" state is 404 or 401 must be called from the
panel with raw `fetch`, not `req`, because `req` treats 401 as global and logs the
operator out (`web/src/lib/api.ts:24-25,48`). `sheetsStatus` and `oidcConfigured`
are the two precedents.

## Mutating a link: the four mandatory steps

Order matters. Skipping a step gives a silent bug - stale cache on another node,
or a webhook lost on restart. `src/api/links_admin.rs:284-308`:

```rust
// 1. build the event
let ev = WebhookEvent { event_type, body: webhook_event_payload(..), tenant_id: p.tenant };
// 2. materialize durable deliveries
let rows = st.webhooks.lifecycle_deliveries(p.tenant, &ev).await;
// 3. write the mutation and the outbox rows in ONE transaction
if st.store.put_link_tx(p.tenant, id, &rec, &rows).await.is_err() {
    return StatusCode::SERVICE_UNAVAILABLE.into_response();
}
// 4. invalidate cache, then emit in-memory
st.cache.invalidate(id).await;
st.webhooks.emit_if_in_memory(ev);
```

Sibling invalidations for other resources: a domain mutation calls
`st.host_router.invalidate(&domain.host).await`
(`src/api/domains.rs:189,242`); a membership or OIDC-config mutation calls
`st.oidc_tenants.invalidate(p.tenant)` (`src/api/invites.rs:315,362`).

## New field on a persisted type

1. `#[serde(default)]` on the field - `#[serde(default, skip_serializing_if = "Option::is_none")]`
   if it is an `Option`. `#[serde(skip)]` if it is PII that must stay in memory.
2. Doc comment stating that old blobs must deserialize rather than fail.
3. Regression test in the same file's `#[cfg(test)] mod`, deserializing the
   literal old JSON and asserting the default.
4. Postgres: append `ALTER TABLE x ADD COLUMN IF NOT EXISTS y TYPE` to the end of
   the `init_schema` array, with a comment naming the feature.
5. LMDB: nothing, if it is inside an existing JSON value.
6. If the panel reads it: type in `web/src/lib/types.ts` (`?` if the Rust side
   skips `None`) and wherever it is rendered.

## New env var

1. Read it in `main.rs` (or in the feature's `X::from_env()`), with the tolerant
   parse idiom and an explicit default.
2. Store the typed value in an `AppState` field with a doc comment naming the var.
3. Log the resulting mode at boot: `enabled (...)` / `disabled (set QUARK_X to
   enable)`. A missing security-relevant value prints `WARNING:` explaining the
   consequence.
4. If parsing has a real rule, extract a pure `fn f(raw: Option<String>) -> T`
   and unit-test it inline.
5. If it needs validation or normalization, split `from_env` / `from_parts` so
   tests never `set_var`.
6. Row in **both** `docs/CONFIGURATION.md` and `docs/CONFIGURATION.PT_BR.md`, with
   the "Default" column.
7. If it is a secret, also `fly secrets set` - never in `fly.toml`.

## New Store method

1. Signature on the trait in `src/store/mod.rs`, `tenant: TenantId` first (unless
   it is a global hash/host lookup, which then needs a doc comment saying so).
2. Implement in `lmdb.rs` and `postgres.rs`. Postgres tenant-owned access goes
   through `with_read!` / `with_write!`. LMDB tenant-owned keys go through
   `tkey` / `tkey_id`.
3. Where a backend cannot support it: `Err(StoreError::Unsupported)` plus a
   comment saying why it is never reached there. Never `unimplemented!()`.
4. If only one backend can do it, consider a default body on the trait instead,
   documenting who overrides it.
5. If it mutates and must emit a durable event, add the `*_tx` pair.
6. New table? `TENANT_OWNED_TABLES`, the `NOT_FORCED` decision, `reset_for_tests`
   TRUNCATE + sequence RESTART. New LMDB sub-db? Bump `MAX_DBS` and consider
   `TENANT_OWNED_DBS`.
7. Test in `tests/store_it.rs` (LMDB) and `tests/postgres_store_it.rs` (gated,
   `#[file_serial]`).

## New background worker

1. `pub fn spawn_<name>(deps) -> tokio::task::JoinHandle<()>` in the owning
   module, single `tokio::spawn`, `select!` over `rx.recv()` and
   `interval.tick()` only.
2. Capacity / interval / timeout as named consts with doc comments in that module.
3. Store reads only in the ticker arm, wrapped in
   `tokio::time::timeout(SNAPSHOT_TIMEOUT, load)`, keeping the previous snapshot
   on error.
4. Drain on channel close before `break` (copy analytics, not the webhook worker).
5. Errors: `tracing::warn!(error = %e, "<what failed>")` (or `error!` when it
   needs attention), inside a span for the worker. Never panic.
6. Multi-replica? Per-process holder id + `try_acquire_*_lease` each tick.
7. Wire it in `main.rs`: clone the `Arc`s at the call site, bind the handle to
   `let _<name> = ...`.
8. Test the loop directly (`worker_drains_and_writes_on_channel_close` is the
   model), not through `main`.

## New bench

1. `benches/<name>_bench.rs` **and** a `[[bench]] name = "<name>_bench"
   harness = false` block in `Cargo.toml`.
2. `criterion_group!` + `criterion_main!`, `black_box` on inputs and outputs, vary
   the input so it is not const-folded.
3. Async: own multi-thread runtime, setup in `rt.block_on`, measure with
   `b.to_async(&rt)`.
4. Header comment stating what the number measures and what it does not.
5. `cargo bench` is not in CI - run it and report the numbers yourself.

## Documentation map

| Change | Doc to touch |
|---|---|
| New/changed endpoint | `docs/API.md` + `docs/API.PT_BR.md` |
| New env var | `docs/CONFIGURATION.md` + `.PT_BR.md` |
| New feature | spec in `docs/specs/YYYY-MM-DD-<slug>.md` (design specs get a `-design` suffix) and plan in `docs/plans/YYYY-MM-DD-<slug>.md` |
| Architectural decision | `docs/DECISAO-<slug>.md` (pt-BR only, no twin) |
| Operational procedure | `docs/RUNBOOK-<slug>.md` (pt-BR only, no twin) |
| Research / audit | `docs/research/` |

Every user-facing doc has a `.PT_BR.md` twin, and both open with the
language-switch line (`**English** · [Português](X.PT_BR.md)`). Prose follows the
avoid-ai-writing rules: no em-dashes, plain direct technical English, natural
pt-BR on the twin.

## Windows / local dev traps

- `cargo` is not on PATH. Use `~/.cargo/bin/cargo.exe`.
- `docs/DEVELOPMENT.md` suggests
  `export QUARK_KEY=$(od -An -N8 -tu8 /dev/urandom | tr -d ' ')`, which does not
  exist in PowerShell. There it is `$env:QUARK_KEY = "..."` - PowerShell has no
  inline env-var prefix.
- `QUARK_DATA` defaults to `./data` and the repo has a checked-in `data/data.mdb`
  at the root. `cargo run` from the root writes there. Tests must use
  `tempfile::tempdir()`.
- LMDB's `lock.mdb` is held by a live process on Windows: a forgotten `cargo run`
  blocks the next `open()`.
- **The LMDB tests exhaust the Windows pagefile (LUC-137).** `MAP_SIZE_BYTES` is
  64 GiB, and on Windows an mmap counts against the commit charge the moment the
  env opens, unlike Linux where the reservation is lazy. `cargo test` runs 30
  test binaries in parallel, several of which open LMDB envs, so the commit
  charge runs out and `store::lmdb::tests::*` fail with OS error 1455
  (`ERROR_COMMITMENT_LIMIT`, "the paging file is too small").
  The failure does not stay inside the tests: allocation also fails inside
  `rustc`, which surfaces as `E0786 found invalid metadata files` and
  `crate X required to be available in rlib format`, and as builds dying with no
  message at all. Do not chase those as a corrupt `target/` - check for 1455
  first. Workaround until LUC-137 lands: `cargo test -j 4 -- --test-threads=1`,
  and rely on CI (Linux) for the LMDB tests.
- Never run two cargo commands against the same `target/` at once. Give clippy
  its own: `CARGO_TARGET_DIR=target-clippy cargo clippy --all-targets -- -D warnings`.
  Mixing check artifacts (rmeta) with build artifacts (rlib) breaks the test
  build with the same misleading rlib errors.
- `rust-toolchain.toml` says only `channel = "stable"`, so a stale local rustup
  can produce clippy results that differ from CI.
