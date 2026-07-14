# Atomic Postgres analytics (remove the per-link hotspot) — design (scale-audit #4)

**Branch:** `feat/scale-hardening` (stacked). NOT merged.
**Closes:** scale-audit gap #2 — the Postgres analytics sink does a read-modify-write of a whole JSONB aggregate blob under `pg_advisory_xact_lock(id)`, serializing every batch for a hot link and rewriting the entire blob each flush (O(blob) write amplification). ClickHouse already avoids this (append-only + aggregate-on-read); this makes the Postgres path scale for hot links too.

## Current (the hotspot)
`src/store/postgres.rs` `record_batch`: per link id, `pg_advisory_xact_lock(id)` then `SELECT agg` → `apply()` → upsert whole `stats.agg` blob, then `SELECT recent` → append/trim → upsert whole `events.recent` blob. Two full-blob RMWs per id per flush, serialized by the advisory lock.

## New design — atomic increments + append-only events, no advisory lock

### Schema (additive, idempotent CREATE TABLE IF NOT EXISTS)
- `click_counters (id BIGINT, dimension TEXT, bucket TEXT, count BIGINT NOT NULL, PRIMARY KEY (id, dimension, bucket))` — one row per (link, dimension, bucket). Dimensions: `total` and `bots` (bucket = `''`), and `day`/`country`/`device`/`os`/`browser`/`referer`/`city`/`variant` (bucket = the map key).
- `stats_meta (id BIGINT PRIMARY KEY, first_ts BIGINT NOT NULL, last_ts BIGINT NOT NULL)` — the two min/max values (not counts).
- `click_events (seq BIGSERIAL PRIMARY KEY, id BIGINT NOT NULL, ts BIGINT NOT NULL, referer TEXT, country TEXT, user_agent TEXT, city TEXT, variant INT, event_id TEXT NOT NULL DEFAULT '')` with an index on `(id, seq DESC)` — append-only recent-events. (ip/fbc are never persisted, matching `#[serde(skip)]`; `bot` is recomputed on read, matching the current behavior.)
- Keep the old `stats`/`events` CREATE lines in place (idempotent, harmless) but the new code path no longer reads/writes them. Pre-existing blob analytics is NOT auto-migrated; document that a fresh cutover starts new counters (acceptable: Postgres analytics is opt-in and this branch is not yet deployed). A one-time migration script is a follow-up if needed.

### record_batch (no advisory lock)
Reuse `Aggregates::apply` so the derivation logic (is_bot early-return, device/os/browser/referer/city/variant) stays in one place: for each link id in the batch, build a fresh `Aggregates::default()`, `apply()` every event, and the resulting Aggregates IS that batch's DELTA. Then, in one transaction:
- For each non-zero counter (total, bots, and every per_* map entry): `INSERT INTO click_counters (id,dimension,bucket,count) VALUES (...) ON CONFLICT (id,dimension,bucket) DO UPDATE SET count = click_counters.count + EXCLUDED.count`. Atomic increment, no lock, correct under concurrency.
- `INSERT INTO stats_meta (id,first_ts,last_ts) VALUES (...) ON CONFLICT (id) DO UPDATE SET first_ts = LEAST(stats_meta.first_ts, EXCLUDED.first_ts), last_ts = GREATEST(stats_meta.last_ts, EXCLUDED.last_ts)`.
- `INSERT INTO click_events (id,ts,referer,country,user_agent,city,variant,event_id)` one row per event (append-only, no read).
- Retention: after inserting a link's rows, bound `click_events` to the newest `EVENTS_MAX` per id: `DELETE FROM click_events WHERE id=$1 AND seq < (SELECT MIN(seq) FROM (SELECT seq FROM click_events WHERE id=$1 ORDER BY seq DESC LIMIT EVENTS_MAX) t)`. Not a lock hotspot.

Concurrency correctness: two replicas flushing the same hot id now both run atomic `count = count + n` upserts (no lost update, no serialization on an advisory lock). Verified by adapting the existing `record_batch_concurrent_no_lost_updates` test (two stores, assert total == 2*n) — it must still pass, now WITHOUT the advisory lock.

### stats read
- `SELECT dimension, bucket, count FROM click_counters WHERE id=$1` → reconstruct `total`, `bots`, and the per_* maps.
- `SELECT first_ts, last_ts FROM stats_meta WHERE id=$1`.
- `SELECT ... FROM click_events WHERE id=$1 ORDER BY seq DESC LIMIT EVENTS_MAX` → reverse → `recent` (rebuild ClickEvent; ip/fbc None; recompute bot via is_bot as today).
- Return `None` when the link has no counters and no events (matching current "no stats" behavior).

### Idempotency — explicitly out of scope for #4
The atomic increment fixes the LOST-UPDATE / hotspot problem. It does NOT make re-processing an event idempotent (a replay would double-count). Current ingestion is at-most-once (no replay), so this is fine today. Note in the doc: if at-least-once ingestion is added later, increments must dedup on `ClickEvent.event_id` (a `processed_events` table) — that is a separate follow-up, unblocked by #2 which already carries a stable event_id.

## Files
- `src/store/postgres.rs` — new tables in the migration; rewrite the `AnalyticsSink::record_batch` and `stats` impls; keep `row_to_...` helpers as needed.
- `tests/postgres_analytics_it.rs` — keep/adapt the concurrency + retention tests to the new schema; assert no advisory lock is needed for correctness.
- `docs/ANALYTICS.md` + `.PT_BR.md` — a short note that the Postgres sink uses atomic counters + append-only events (scales for hot links), the idempotency caveat, and that ClickHouse remains the recommended sink for very high volume.

## Testing (gated on QUARK_TEST_DATABASE_URL)
Use the live Postgres at `postgres://quark:quark@127.0.0.1:5432/quark`. Run `QUARK_TEST_DATABASE_URL=... CARGO_BUILD_JOBS=1 cargo test -j1 -p quark --test postgres_analytics_it --test postgres_store_it`. Must be green: aggregation correctness (per-dimension counts match), concurrent-no-lost-updates (total == 2*n) now WITHOUT the advisory lock, recent retention (<= EVENTS_MAX, newest kept), stats None when empty, bots excluded from per_* but counted in total.

## Global constraints
Code English; no inline `//`; ClickHouse sink unchanged; LMDB sink unchanged; single-node unchanged; docs EN + PT_BR, avoid-ai-writing; no merge to main; Rust tests `-j1`; Postgres tests gated.
