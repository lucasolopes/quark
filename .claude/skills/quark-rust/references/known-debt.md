# Known debt and internal inconsistencies

Where the repo does the same thing two ways, this is which side to copy. Do not
"harmonize" these as a side effect of another task; each one is its own change.

All of this is tracked work, not permanent state. The constraint on every item:
**nothing may slow the redirect hot path** (`GET /:code`, the cache, the store
read path). Where a fix touches it, `benches/redirect_bench.rs` numbers before
and after go in the PR.

Epic **LUC-124**. Most of it shipped; what is left is listed under "Still open"
below.

All 13 shipped. Highlights worth remembering: **zero `eprintln!` and zero
`Result<_, String>` left in `src/`**; every error is a `thiserror` enum;
`tracing` + `TraceLayer` replaced the hand-rolled logging; axum 0.8, heed 0.22,
redis 1.x and sqlx 0.9 are current.

Already tracked elsewhere: **LUC-103** covers the silent analytics drop.

Deliberately **out of scope** until a benchmark justifies them: moving LMDB point
reads to `spawn_blocking`, swapping the global allocator, and a per-request span
on the redirect path.

## Still open (do not extend the legacy form)

| Legacy in the repo | Required instead |
|---|---|
| `StoreError` used as the `AnalyticsSink` error | its own error type |
| 10 `env::var(..).unwrap_or_default()` in `keycloak/mod.rs:82-143` | warn and stay off, like `OidcConfig::from_env` now does |
| ~26 `expect()` sites | keep, but each needs `#[expect(clippy::expect_used, reason = ..)]` before the lint can be turned on |

Full rules and the module-at-a-time conversion procedure:
[errors-and-observability.md](errors-and-observability.md).

## Two ways to do the same thing

| Topic | Dominant / correct | Outlier - do not copy |
|---|---|---|
| `async_trait` on a data-plane trait | `#[async_trait::async_trait]` full path inline (12 sites) | `use async_trait::async_trait;` + `#[async_trait]` (9 sites, the newer HTTP/DNS seams) |
| Trait bound for a pluggable backend | `Send + Sync + 'static` (Store, AnalyticsSink, CacheTier) | `Send + Sync` only (Dns, KeycloakAdmin, SheetsApi) |
| Tenant scoping in production | explicit `st.store.m(p.tenant, ..)` (136 sites) | `ScopedStore` / `for_tenant` (tests only, 0 uses in `src/api`) |
| CSRF check | `csrf_guard(&headers)` (4 sites) | manual `headers.get(HEADER_CSRF).is_none()` in `oidc_logout` |
| OAuth `state` comparison | `constant_time_eq` (sheets, slack) | `!=` in the OIDC callback (`oidc_login.rs:169-172`) - the most sensitive of the three |
| Boolean env var | `.map(\|v\| v != "0")` (3 sites, and what `CONFIGURATION.md` documents) | `matches!(.., Ok("true") \| Ok("1"))` (`keycloak/mod.rs:87-90`) |
| RNG panic message | `"system RNG must be available"` (7 sites) | `"system randomness source unavailable"` (`auth.rs:74`, `secretbox.rs:182`) |
| Periodic task | named `spawn_*` in the owning module (5 workers) | inline `tokio::spawn` block in `main.rs` (sheets sync ~70 lines, session GC, retention purge) |
| Gated-test skip | `else { eprintln!("skip: ..."); return; }` (~155) | silent `else { return; }` (26), and one `.unwrap()` in `postgres_analytics_it.rs:311` |
| Integration test filename | `<area>_it.rs` (27 files) | `tenant_isolation.rs`, `tenant_enforcement.rs`, `store_trait.rs` |
| Suppression attribute | `#[expect(lint, reason = "...")]` for new ones | the existing `#[allow]`s with no reason - leave them |
| Module error type | `#[derive(thiserror::Error)]` | a hand-written `Display` + `Error` impl, or `Result<_, String>` |
| Runtime log line | `tracing::warn!(field = %v, "message")` | `eprintln!` (none left in `src/`; do not reintroduce) |

## Real gaps

- **Silent event drop.** `src/api/links.rs:1333` does
  `let _ = st.analytics_tx.try_send(ev);` with no log or counter, so a slow
  consumer erases clicks and conversions without a trace. The correct shape is
  `try_emit` in `src/webhooks/delivery.rs:91-100`. For the hot path, count drops
  in an `AtomicU64` and log an aggregate on the 5s tick rather than one line per
  drop.
- **15 `map_err(|_| ..)`** discard the cause. Deliberate in `secretbox.rs` (a
  decrypt failure must not explain why); worth carrying the cause in `import.rs`
  / `pixel.rs` when the variant can hold it.
- **LMDB blocking calls inside `async fn`** without `spawn_blocking`, including
  `write_txn` + `commit` (which fsyncs). Fine for a point read; a scan or a large
  commit is not.
- **Real sleeps in tests**: `tests/pixel_forward_it.rs:375` (5.5s of wall clock),
  `src/webhooks/delivery.rs:2006` (300ms). `src/cache/mod.rs:299` shows the right
  shape (`start_paused`).
- **Three integration files still miss the `_it.rs` suffix**:
  `tenant_isolation.rs`, `tenant_enforcement.rs`, `store_trait.rs`.
- **`env::var(SECRET).unwrap_or_default()`** still turns missing config into an
  empty string in `src/keycloak/mod.rs:82-143` (10 vars). `OidcConfig::from_env`
  was fixed: it warns and leaves the feature off.
- **`web/src/lib/variants.ts:9-10`** mirrors `MAX_VARIANTS` with a stale comment
  pointing at `src/api.rs`, which is now `src/api/links.rs`.
- **`web/.npmrc` declares `save-exact=true`** but `package.json` is dominated by
  `^` ranges (~45 vs 4 exact). Declared and actual disagree; pick one explicitly
  rather than ignoring it.
- **`web/tsconfig.app.json` does not set `"strict": true`.** Handle `null` /
  `undefined` explicitly; do not assume the compiler catches it. Turning strict on
  is a broad change, not a PR-sized tweak.
- **`benches/redirect_bench.rs:81-113`** is the last `AppState` struct literal in
  the repo and already divergent (`real_ip_header` hardcoded to
  `"cf-connecting-ip"`).
