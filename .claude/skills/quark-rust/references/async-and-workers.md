# Async, tokio, workers, concurrency

## Background workers

Every worker lives in the module that owns the behaviour, as a single
`tokio::spawn` inside a `spawn_*` function. `main.rs` is the only caller.

```rust
/// Drains the analytics channel: flushes on BATCH, on the 5s tick, and on
/// channel close.
pub fn spawn_worker(
    sink: Arc<dyn AnalyticsSink>,
    mut rx: tokio::sync::mpsc::Receiver<ClickEvent>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut buf = Vec::with_capacity(BATCH);
        let mut ticker = tokio::time::interval(Duration::from_secs(FLUSH_SECS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                maybe = rx.recv() => match maybe {
                    Some(ev) => { buf.push(ev); if buf.len() >= BATCH { flush(&sink, &mut buf).await; } }
                    None => { flush(&sink, &mut buf).await; break; }   // drain, then exit
                },
                _ = ticker.tick() => flush(&sink, &mut buf).await,
            }
        }
    })
}
```

Rules:

- `pub fn spawn_<name>(args) -> tokio::task::JoinHandle<()>`, one `tokio::spawn`,
  shared resources arriving as already-cloned `Arc`s.
- `tokio::select!` only over cancel-safe futures - `rx.recv()` and
  `interval.tick()`. `src/analytics/mod.rs:619-634` is the canonical shape.
- Tickers: `tokio::time::interval` + `set_missed_tick_behavior(Delay)`.
- **Never `tokio::spawn` inside a handler.** Publish to the existing channel and
  let the already-spawned worker handle it. No handler in `src/api/` spawns.
- The five production workers are `analytics::spawn_worker`,
  `webhooks::delivery::spawn_webhook_worker`, `spawn_webhook_relay`,
  `health::spawn_link_checker`, `invalidate::spawn_invalidation_subscriber`.
  `main.rs` adds three more as named `spawn_*` functions (`spawn_sheets_sync`,
  `spawn_session_gc`, `spawn_analytics_purge`). A new periodic loop follows the
  same shape: a named function, never an inline `tokio::spawn` block in `main`.
- Never panic in a worker: log JSON and continue. The `JoinHandle` is bound to
  `let _worker = ...` (deliberate discard), so a panicking task dies silently -
  and with `panic = "abort"` in release the whole process dies.

## Channels

Bounded, capacity in a named const, created in `main.rs`.

- `ANALYTICS_CHANNEL_CAPACITY = 10_000` (`main.rs:18`),
  `WEBHOOK_CHANNEL_CAPACITY = 1024` (`webhooks/delivery.rs:22`). No
  `unbounded_channel` anywhere in `src/`.
- From the hot path always `try_send`, **never `send().await`**: the redirect path
  must not take backpressure. Event delivery is best-effort and fail-open.
- Report the drop. `src/webhooks/delivery.rs:81-101` (`try_emit`) is the shape to
  copy: match the `Err`, emit a JSON line, return a bool the caller can act on.
  `src/api/links.rs:1333` does `let _ = st.analytics_tx.try_send(ev);` with no
  log - known debt. For a new producer, count drops in an `AtomicU64` and log an
  aggregate on the worker's tick rather than one line per dropped event.

## Snapshots: never hit the store on the event path

A worker keeps an in-memory snapshot (typically `Vec<(TenantId, Vec<T>)>`) and
refreshes it **only** in the ticker arm, wrapped in a timeout:

```rust
let load = async { store.list_webhooks_all().await };
match tokio::time::timeout(SNAPSHOT_TIMEOUT, load).await {
    Ok(Ok(fresh)) => subs = fresh,
    Ok(Err(e)) => tracing::warn!(error = %e, "webhook snapshot refresh failed"),
    Err(_) => tracing::warn!("webhook snapshot refresh timed out"),
    // previous snapshot stays intact
}
```

On error or timeout the previous snapshot is left alone. Reason recorded in the
code: a stalled store (exhausted Postgres pool) would stagnate the flush and fill
the bounded analytics channel, hitting the hot path; and zeroing a good snapshot
on a transient error would silently degrade webhooks.
Evidence: `src/analytics/mod.rs:578-596,656-683`, `src/webhooks/delivery.rs:218-280`.

Dispatch filters by tenant before acting:
`subs.iter().find(|(t, _)| *t == ev.tenant_id)`.

## Locks

- Small, synchronous critical sections use **`std::sync::Mutex`**, not
  `tokio::sync::Mutex`: rate limiter (`src/abuse/ratelimit.rs:65-77`), alert
  counter (`src/analytics/mod.rs:411-424`), Keycloak token cache
  (`src/keycloak/client.rs:61-73`).
- The guard **never** crosses an `.await`. Extract or clone the value out of the
  lock scope first:

```rust
let cached = self.token.lock().expect("token mutex").as_ref().map(|c| c.token.clone());
match cached { Some(t) => t, None => self.fetch_token().await? }
```

- `tokio::sync::RwLock` appears exactly once, for JWKS, because the guard
  coexists with async verification and an HTTP refetch (`src/oidc.rs:573`).
- Shared flags between worker and hot path are `Arc<AtomicBool>` with
  `Ordering::Relaxed`. The L2 circuit breaker is `AtomicU32`/`AtomicU64` only,
  explicitly "no locks" (`src/cache/mod.rs:59-91`). Read a gate once into a local
  when it decides data ownership further down.

## Caches

- Hot-path L1 uses `moka::sync::Cache as Moka`:
  `Moka::builder().max_capacity(n).time_to_live(Duration::from_secs(ttl)).build()`
  with synchronous `get`/`insert`/`invalidate`.
- `moka::future::Cache` only when the value must be built asynchronously - the
  per-tenant `OidcRuntime` cache, which does discovery + JWKS
  (`src/oidc.rs:717-747`).
- `Cache::invalidate_local` and `HostRouter::invalidate_local` are declared
  `async` with no `await` in the body, for call-site symmetry with the publishing
  `invalidate`. Do not read that `async` as a sign of IO.

## Timeouts on every outbound call

Two layers, both with named consts.

```rust
/// Health probes hit user-supplied URLs, so no redirects (SSRF) and a short budget.
fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(PROBE_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("reqwest client builds")
}
```

- Never `reqwest::Client::new()` in production and never a client without
  `.timeout(...)`. Build it in a named function in the owning module
  (`build_client`, `http_client`, `keycloak_client`, `reqwest_client`) with a doc
  comment justifying the timeout and redirect policy. Clients that fetch a
  user-supplied URL also set `.redirect(Policy::none())`.
  Existing budgets: 5s webhook delivery, 5s pixel forward, 10s health/OIDC,
  30s Sheets/Keycloak.
- No builder currently sets `connect_timeout`. For a **new** client, add a
  `connect_timeout` smaller than the total budget, so a destination that accepts
  TCP and stalls in the TLS handshake cannot consume the whole budget.
- Operations without their own timeout (redis, resolver, snapshot loads) are
  wrapped in `tokio::time::timeout(CONST, fut)`. A timeout counts as a failure
  (opens the breaker) and never becomes a 500. Existing: `L2_OP_TIMEOUT` 100ms,
  `PUBLISH_TIMEOUT` 100ms, `LOOKUP_TIMEOUT` 5s, `SNAPSHOT_TIMEOUT` 3s.

## Fail-open at the async edges

`Cache` L2, invalidation publish, pixel forward, webhook delivery and retention
purge all fail open: log a JSON line and continue (breaker + fallback to store,
dead-letter, or keep the previous snapshot). The `CacheTier` trait returns
`Result<_, TierError>` but `Cache::get`/`invalidate` never propagate it - "tier
errors never propagate to the caller" is a declared module invariant
(`src/cache/mod.rs:30-38`).

The rate limiter also fails open on a Valkey error (`check_with_limit` returns
`true`) and webhook delivery only logs when the attempt budget is exhausted.
Both are documented decisions. Changing them is a product decision, not a bugfix.

## CPU-bound work

argon2 is memory-hard and must not stall the runtime that serves redirects:

```rust
let ok = tokio::task::spawn_blocking(move || crate::password::verify_password(&submitted, &hash))
    .await
    .unwrap_or(false);          // a JoinError degrades to "wrong password"
```

For hashing: `match ... { Ok(Ok(h)) => h, _ => return 500 }`. Never `.unwrap()`
the `JoinHandle`. Evidence: `src/api/links.rs:581-590,1093-1101`,
`src/api/links_admin.rs:597`.

LMDB (`heed`) calls run synchronously inside `async fn` without `spawn_blocking`.
For a point `get` that is fine - it is a memory-mapped read in the microsecond
range, and dispatching to a blocking pool would cost more. But an LMDB operation
that **scans** (`list_links`, `search_links`, `list_aliases`, bulk import) or
commits a large write is potentially long work and belongs in
`tokio::task::spawn_blocking`. Decide with `benches/redirect_bench.rs`, not by
guessing.

## Retry and backoff

Exponential with jitter from `getrandom`, saturating, capped:

```rust
let base = BACKOFF_BASE_MS.saturating_mul(1u64 << attempt.min(16));
let mut b = [0u8; 1];
let jitter = if getrandom::fill(&mut b).is_ok() { (b[0] as u64) % (base / 2 + 1) } else { 0 };
tokio::time::sleep(Duration::from_millis(base + jitter)).await;
```

Two machines coexist deliberately: in-memory retry (`DELIVERY_ATTEMPTS = 3`,
`BACKOFF_BASE_MS = 200`) and the durable schedule (`MAX_DELIVERY_ATTEMPTS = 8`,
`RELAY_BACKOFF_BASE_SECS = 2`, `RELAY_BACKOFF_CAP_SECS = 600`) written to
`next_attempt_at` in Postgres so it survives a restart. Do not collapse the
durable one into an in-process retry. Neither honours HTTP `Retry-After` - a real
gap, but its own task.

## Multi-replica periodic tasks

A periodic task that must not run on every replica generates a per-process holder
id from 8 random bytes (`format!("chk_{}", crate::hex(&hb))`) and, each tick,
calls `store.try_acquire_<x>_lease(&holder, ttl)`: `Ok(false)` -> `continue`,
`Err` -> log JSON and `continue`. TTL derives from the period (`period * 2`, or
`secs.min(300)`). Sheets sync releases the lease at the end of the tick so
on-demand sync is not blocked. Evidence: `src/health.rs:294-317`,
`src/main.rs:528-548,605-607`.

## Composition in main.rs

`main.rs` is the only place that creates channels, HTTP clients, `Arc<AppState>`
and spawns workers. Each worker receives `store.clone()` / `state.clone()`
explicitly at the call site, and the returned `JoinHandle` is bound to an
underscore-prefixed binding (`let _worker = ...`, `let _checker = ...`) marking
the intentional discard. `AppState` is wrapped in `Arc::new(AppState { ... })`
exactly once and handed to `router(state)`.

## Shutdown: what does not exist

`axum::serve(...)` is called **without** `.with_graceful_shutdown(...)`, no
`select!` has a cancellation arm, and there is no `tokio::signal` usage anywhere -
even though the `signal` feature is enabled in `Cargo.toml`. Consequences to know:

- On SIGTERM (a Fly deploy) the analytics buffer and the webhook queue are lost,
  and `panic = "abort"` means no destructors run.
- The analytics drain path (`rx.recv() == None` -> final flush -> `break`) is
  only exercised in tests, because the `Sender` lives inside the `Arc<AppState>`
  that is never dropped.

If you work on shutdown, do it wholesale: `with_graceful_shutdown` (ctrl_c +
SIGTERM), drop the `Sender`, and `await` each worker's `JoinHandle` before `main`
returns. The analytics loop is already cancel-safe and already drains on channel
close, so **no new dependency is needed** - do not add `tokio-util` /
`CancellationToken` for this. Do not ship a half-solution in one worker.

Also note: `spawn_webhook_worker` does `None => break` with no drain, unlike
analytics. Analytics is the model; the webhook worker is the debt.

## Log lines

```rust
tracing::error!(error = %e, url = %sub.url, "webhook delivery failed");
```

Structured fields, short English message, level chosen deliberately: `warn!` for
every fail-open degradation, `error!` for something that needs attention, `info!`
for boot and lifecycle. Instrument each worker loop with a span so a delivery
error can be tied back to what produced it.

`src/` has no `eprintln!` left. The only `println!` are CLI output, not logs:
`--version` in `main.rs` (before the subscriber is installed) and the table
`src/bin/calibrate.rs` prints. Everything operational goes through `tracing`.
See [errors-and-observability.md](errors-and-observability.md) for the table.

One thing does not change: **no per-request event on the redirect path.** Handler
store errors stay discarded and mapped to a status.
