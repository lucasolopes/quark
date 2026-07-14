# Cross-node invalidation via Valkey pub/sub — design

**Status:** approved (scale-audit item #1; decisions recorded autonomously per the standing scale-hardening go-ahead).
**Branch:** `feat/pubsub-invalidation` (off `main@f50c745`). NOT merged.
**Closes:** scale-audit gaps #4 (cache patch/delete leaves stale L1 on other replicas ≤60s) and blocklist propagation ≤ TTL — both with ONE channel.

## Problem

With N replicas on Postgres + Valkey:
- A `patch`/`delete` on node A clears A's L1 (moka) and DELs the shared L2 (Valkey), but **node B's L1 still holds the stale `Record`** until its 60s L1 TTL lapses. The L2 DEL never reaches an already-populated L1. Result: up to 60s of stale redirects on B (`src/cache/mod.rs:172-183`).
- A blocklist add/remove on node A zeroes A's snapshot and DELs the shared key, but **other nodes keep their in-memory snapshot until their TTL** (default 60s). A newly-blocked domain is not immediately effective cluster-wide (`src/abuse/blocklist.rs:15,47-64`).

## Goal

Make both invalidations propagate promptly across all replicas, keeping the per-node TTL as the safety net (so a missed pub/sub message still self-heals within the TTL).

## Design

### One channel, best-effort fan-out
A single Valkey pub/sub channel **`quark:invalidate`**. Messages are tiny text payloads:
- `link:<id>` — drop the L1 cache entry for that link id everywhere.
- `blocklist` — force every node to reload its blocklist snapshot on the next check.

Valkey pub/sub is at-most-once (no persistence): a disconnected node misses a message, but its existing TTL (L1 60s / blocklist 60s) still bounds staleness. So this turns "≤60s always" into "prompt in the common case, ≤60s worst case" — strictly better, with the TTL as the documented backstop. This matches the research recommendation (short TTL + pub/sub invalidation).

### Publisher — reuse the existing multiplexed connection
A small `Invalidator` holds an `Option<redis::aio::MultiplexedConnection>` (PUBLISH is a normal command, safe on the multiplexed connection already used for rate-limit/blocklist) plus the channel name. `publish(msg)` is best-effort and fail-open: a Valkey error is logged and ignored, never blocks the caller.

`Invalidator` is created once in `main.rs` from `QUARK_VALKEY_URL` (clone of the control connection) and injected into both `Cache` and `Blocklist` as `Option<Arc<Invalidator>>`. When Valkey is unset (single-node), the Invalidator is `None` and nothing publishes/subscribes — single-node behavior is unchanged.

### Subscriber — dedicated connection, local-only invalidation
`spawn_invalidation_subscriber(url, state: Arc<AppState>)` runs a background task (mirrors the analytics/webhook worker shape):
- Opens a **dedicated** pub/sub connection (Redis SUBSCRIBE monopolizes a connection; it cannot share the multiplexed one). `redis::Client::get_async_pubsub()`.
- `SUBSCRIBE quark:invalidate`; loops on the message stream.
- On `link:<id>` → `state.cache.invalidate_local(id)` (drop L1 only).
- On `blocklist` → `state.blocklist.invalidate_local()` (zero the snapshot's `loaded_at` so the next check reloads).
- On a malformed payload → log and ignore. On stream error / disconnect → log, back off, and reconnect (the TTL covers the gap while reconnecting).

### Loop avoidance (critical)
- The request handlers keep calling the existing `Cache::invalidate` / `Blocklist::invalidate`, which now ALSO publish.
- The subscriber calls NEW local-only methods that do **not** publish:
  - `Cache::invalidate_local(&self, id)` — `self.hot.invalidate(&id)` only (no L2, no publish).
  - `Blocklist::invalidate_local(&self)` — zero the snapshot `loaded_at` only (no L2 DEL, no publish).
- The originating node also receives its own message and calls the `_local` variant — a harmless no-op (already invalidated). No self-filtering needed because `_local` never re-publishes.

## Components / files
- New: `src/invalidate.rs` — `Invalidator { conn, ... }` with `publish(&self, msg: &str)`; `INVALIDATION_CHANNEL` const; `spawn_invalidation_subscriber(url, state)`.
- Modify: `src/cache/mod.rs` — `Cache` gains `Option<Arc<Invalidator>>`; `invalidate` publishes `link:<id>`; add `invalidate_local`.
- Modify: `src/abuse/blocklist.rs` — `Blocklist` gains `Option<Arc<Invalidator>>`; `invalidate` publishes `blocklist`; add `invalidate_local`.
- Modify: `src/main.rs` — build the `Invalidator` from `QUARK_VALKEY_URL`, inject into `Cache::new`/`Blocklist::new`, and after `AppState` is built, `spawn_invalidation_subscriber` when Valkey is configured.
- Modify: `src/lib.rs` — expose the module.

## Testing
- Unit: `Invalidator::publish` with no connection is a no-op; message parse (`link:<id>` valid/invalid, `blocklist`).
- Gated integration (`QUARK_TEST_VALKEY_URL`/`QUARK_VALKEY_URL`, like `valkey_tier_it`): two `Cache` instances sharing one Valkey; A `invalidate(id)` → B's L1 (pre-populated) is dropped after the message is delivered. Same for two `Blocklist` instances: A add domain → B reloads and blocks. Assert the single-node (no Valkey) path still invalidates locally and never touches Valkey.
- Live check: docker Valkey + two quark processes, patch on one, redirect on the other returns the new value promptly (not after 60s).

## Global constraints
Code English; no inline `//`; best-effort/fail-open (never block the hot path or the request on a Valkey error); single-node (no `QUARK_VALKEY_URL`) behavior byte-for-byte unchanged; the per-node TTL remains the backstop; no merge to main; avoid-ai-writing on prose; Rust tests `-j1`; Valkey tests gated.
