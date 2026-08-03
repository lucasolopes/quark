# Testing

## Where a test goes

| What you are testing | Where |
|---|---|
| Pure logic (parser, codec, permute, payload builder, default resolution) | `#[cfg(test)] mod tests { use super::*; }` at the bottom of the same `src/` file (39 such modules) |
| `src/api` helpers | `src/api/tests.rs`, declared by `#[cfg(test)] mod tests;` in `src/api/mod.rs:143-144` |
| Anything crossing HTTP or a real backend | `tests/<area>_it.rs` |
| Throughput of a hot path | `benches/<name>_bench.rs` |

New integration files use the `_it.rs` suffix. Three legacy files miss it
(`tenant_isolation.rs`, `tenant_enforcement.rs`, `store_trait.rs`) - do not add a
fourth. Each `tests/*.rs` is a separate crate: repeat the `use` statements and
declare `mod common;` to use the builder.

## AppState in a test

Never a struct literal. `AppState` has ~26 fields and grows.

```rust
mod common;

let state = common::TestState::new(store, sink)
    .multi_tenant(true)
    .admin_token(Some("t".into()))
    .build();                       // -> Arc<AppState>
let app = quark::api::router(state);
```

Defaults are the OSS single-tenant shape - assume them, do not restate them:
`key = 0x1234`, `signing_key = [0u8; 32]`, `admin_token = None`,
`ratelimiter = RateLimiter::disabled()`, `block_private = true`,
`real_ip_header = DEFAULT_REAL_IP_HEADER`, `oidc_configured = false`,
`multi_tenant = false`, `dns = Arc::new(NullDns)`. Derived defaults:
`Cache::new(store, 1000, None)`, `HostRouter::new(store, None, None)`, an
analytics channel of capacity 100 with the receiver dropped, and
`test_webhook_dispatcher()`.

Adding a field to `AppState` means adding field + setter + default in
`tests/common/mod.rs` in the same change.

`tests/common/mod.rs` carries `#![allow(dead_code)]` at the top, because each
integration crate uses only a subset and CI runs `-D warnings`. Rely on it; do
not add per-file allows.

**Do not redefine `test_webhook_dispatcher()` locally.** 12 files still copy it
(pre-builder residue) and 0 use `common::test_webhook_dispatcher` - the shared
helper is the declared correct form, and usually you can simply omit
`.webhooks(..)` since the builder already defaults to it.

## Making a request

Exercise the real `Router` as a `tower::Service`; no socket, no port.

```rust
use tower::ServiceExt;

let resp = app.clone().oneshot(
    Request::post("/admin/links")
        .header("x-admin-token", "t")
        .body(Body::from(r#"{"url":"https://example.com"}"#)).unwrap()
).await.unwrap();
assert_eq!(resp.status(), StatusCode::CREATED);
let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
```

Clone the Router for each request. `TcpListener::bind("127.0.0.1:0")` +
`tokio::spawn(axum::serve(..))` is reserved for simulating **external** services
(GA4/Meta, a webhook receiver, an IdP) - all 6 uses are third-party mocks.

The error contract is locked by status assertions, since no error type is
serialized. Changing an error mapping means updating the matching
`assert_eq!(resp.status(), StatusCode::X)`.

## Helper at the top of the file

```rust
async fn app() -> Router {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));   // must outlive the helper
    let (store, sink) = quark::store::open_backends(dir.path(), false).await.unwrap();
    quark::api::router(common::TestState::new(store, sink).build())
}
```

The `Box::leak` idiom is the project's way of keeping the tempdir alive past the
helper (`api_it.rs`, `analytics_api_it.rs`, `import_it.rs`, `pixels_api_it.rs`,
`tokens_api_it.rs`, `webhooks_api_it.rs`). Plain `tempfile::tempdir()` when the
scope is local. Never write to `./data` - `QUARK_DATA` defaults there and the
repo has a checked-in `data/` at the root.

## Fixtures

Declare short helpers at the top instead of repeating literals: `fn rec(url) -> Record`,
`fn plain_rec(url)`, `fn ev(id, ts)`, `fn outbox_row(..)`, and derive variations
with struct update: `ClickEvent { tenant_id: t, ..ev(id, ts) }`. `Record` has 13
fields and keeps growing; ~33 raw literals across 12 files are recognized debt -
do not add more.

Local consts in SCREAMING_SNAKE at the top: `const KEY: u64 = 0x1234;` (same as
the builder default), `IPHONE_UA` / `ANDROID_UA` / `DESKTOP_UA`,
`const TEST_SECRET: &str = "whsec_...";`. Never hardcode a tenant or domain: use
`quark::tenant::DEFAULT_TENANT` (233 uses) and `quark::domain::SHARED_DOMAIN_ID`.

Non-obvious tests carry a comment stating the behaviour and, for regressions, the
bug and the `LUC-xx` ticket. Function names are long snake_case sentences
describing the expected behaviour, not the method under test.

## Gated backend tests

`cargo test` with no env vars must pass. Tests needing Postgres / Valkey /
ClickHouse read `QUARK_TEST_DATABASE_URL` / `QUARK_TEST_VALKEY_URL` /
`QUARK_TEST_CLICKHOUSE_URL` and **return** when absent. Never `#[ignore]`, never
`unwrap()` on the var, never panic.

`QUARK_TEST_DATABASE_URL` must point at a **non-superuser** role (compose ships
`quark_test`, not `quark`). Postgres exempts superusers from RLS, so a
superuser URL makes `FORCE ROW LEVEL SECURITY` — the cloud-mode tenant
isolation mechanism — untested, and the isolation tests pass vacuously.
`cloud_force_rls_blocks_raw_sql_without_tenant_predicate` asserts this.

```rust
async fn fresh() -> Option<PostgresStore> {
    let url = std::env::var("QUARK_TEST_DATABASE_URL").ok()?;
    let s = PostgresStore::open(&url, false).await.ok()?;
    s.reset_for_tests().await.ok()?;
    Some(s)
}

#[tokio::test]
#[file_serial]
async fn links_are_isolated_per_tenant() {
    let Some(store) = fresh().await else {
        eprintln!("skip: QUARK_TEST_DATABASE_URL not set");
        return;
    };
    // ...
}
```

- Always print the skip reason. A silent skip is indistinguishable from a pass,
  so a backend that stopped being covered would never show up in the output.
- Never read production vars (`QUARK_DATABASE_URL`, `QUARK_VALKEY_URL`) in a test,
  and never `set_var`: `open_backends` reads `QUARK_DATABASE_URL`, so setting it
  would swap the backend under the LMDB tests. The separate `QUARK_TEST_` prefix
  exists precisely so the suite can never point at a real deployment.
- Reset shared state: `PostgresStore::reset_for_tests()` (TRUNCATE all tables +
  RESTART the `quark_*_seq` sequences + re-seed tenant 0) or
  `ClickHouseSink::reset_for_tests()`. The canonical place is inside `fresh()`.
- **`#[tokio::test]` + `#[file_serial]`**, always without a key. 188 occurrences,
  0 uses of `#[serial]` or a keyed variant. Reason documented in
  `Cargo.toml:41-48`: `cargo test` runs the integration binaries in parallel
  against the *same* Postgres, and plain `#[serial]` only serializes within one
  binary, so two binaries would race on DDL / `FORCE ROW LEVEL SECURITY` /
  `TRUNCATE`. The `file_locks` feature of `serial_test` gives a cross-process lock.
  `docs/DEVELOPMENT.md` documents the same rule.
- CI sets all three vars against docker services, so a gated test you write does
  run on the PR. Locally, `docker compose` brings up the same three backends;
  serialize the binaries with `cargo test -- --test-threads=1` or
  `cargo test --test <file>`.
- If Postgres auth fails with `role "quark" does not exist` while the container
  is healthy, something native is already listening on 5432. Docker publishes on
  `::` and a native server on `127.0.0.1` wins the connection, so the test talks
  to the wrong server. Publish the container on a free port (`55432`) and point
  `QUARK_TEST_DATABASE_URL` at it rather than debugging the container.

## Async tests and time

- `#[tokio::test]` plain (477 occurrences).
- `#[tokio::test(start_paused = true)]` only when the test depends on time
  advancing. `src/cache/mod.rs:299` is the canonical example: a 3600s TTL tested
  instantly. The `tokio/test-util` dev feature is already enabled.
- **Never a fixed `sleep` to wait for an async effect.** Poll with a deadline:

```rust
let deadline = std::time::Instant::now() + Duration::from_secs(5);
loop {
    if condition { break; }
    assert!(std::time::Instant::now() < deadline, "invalidation never arrived");
    tokio::time::sleep(Duration::from_millis(50)).await;
}
```

Real sleeps still exist (`tests/pixel_forward_it.rs:375` sleeps 5.5s,
`src/webhooks/delivery.rs:2006` 300ms) - do not copy them. For a new test whose
subject is a TTL, expiry, backoff, or rate-limit window, use `start_paused`. For
cross-process propagation over Redis pub/sub, virtual time does not help: use the
deadline poll.

## Fakes

Implement the trait inline in the test module, named after the behaviour:

```rust
struct FailingTier;
#[async_trait::async_trait]
impl CacheTier for FailingTier {
    async fn get(&self, _id: u64) -> Result<Option<Record>, TierError> { Err(TierError("boom".into())) }
}
```

`FailingTier` / `HangingTier` at `src/cache/mod.rs:265-314`. Count calls with
`AtomicU32` / `Mutex`. For an external HTTP dependency, spin up a local axum
server on port 0 and return `(format!("http://{addr}"), captured_state)` with the
state in an `Arc<Mutex<Vec<..>>>` (`tests/pixel_forward_it.rs:30-56`,
`tests/webhook_outbox_it.rs:74-89`). Do not add an HTTP mocking crate.

When a production guard would correctly block the test (SSRF against a localhost
server), do not loosen the guard: replicate the injected-predicate pair, like
`deliver_to_matching` (production) / `deliver_to_matching_guarded(.., is_blocked)`
(tests), keeping one end-to-end test that uses the real predicate.

## Benches

criterion with `harness = false`. A new bench needs **both** the file and a
`[[bench]]` block in `Cargo.toml`, otherwise cargo tries the default test harness
and the bench arguments do not work.

```toml
[[bench]]
name = "redirect_bench"
harness = false
```

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};
fn bench_encode(c: &mut Criterion) { /* black_box inputs and outputs */ }
criterion_group!(benches, bench_encode);
criterion_main!(benches);
```

Vary the input so it is not const-folded (`id.wrapping_add(1) & MAX_ID`). Use
`c.benchmark_group("name")` + `group.bench_function(..)` + `group.finish()` when
comparing variants. Async benches build their own runtime
(`Builder::new_multi_thread().worker_threads(4).enable_all().build()`), do setup
inside `rt.block_on(async { .. })` returning the handle tuple, and measure with
`b.to_async(&rt).iter(|| ...)` - the criterion `async_tokio` feature is already
on. Document at the top of the file what the number measures and what it does
**not** (contention).

`cargo bench` is not in CI. The `AppState` literal in
`benches/redirect_bench.rs:81-113` is the last one in the repo and already
divergent - if you touch it, move it to the builder.
