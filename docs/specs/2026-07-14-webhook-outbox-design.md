# Durable webhook delivery (Postgres outbox + leased relay) — design (scale-audit #3)

**Branch:** `feat/scale-hardening` (stacked). NOT merged.
**Closes:** scale-audit gap #1 — webhook delivery is an in-memory best-effort mpsc that drops on a full channel, loses everything on restart, has no dead-letter, no cross-node coordination, and only a random non-persisted dedup id. This makes lifecycle-event delivery durable, retried across restarts, scaled across replicas, and dedupable, on the Postgres backend.

## Scope and split (important)
- **Postgres backend, LIFECYCLE events** (`link.created`, `link.updated`, `link.deleted`, `link.expired`): these are low-volume and durability matters. They go through a durable Postgres outbox + a leased relay worker. THIS is the durable path.
- **`link.clicked`**: high-volume and emitted on the redirect HOT PATH. It STAYS on the existing in-memory best-effort channel (a synchronous DB insert on every redirect would violate the hot-path invariant). Documented as best-effort by design.
- **LMDB backend** (single-node): everything stays on the existing in-memory channel. The outbox is Postgres-only. `main.rs` selects the path by backend.
- **Residual dual-write window** (honest): the delivery rows are inserted right after the link mutation commits, not in the same transaction (the `Store` trait would otherwise have to know about webhooks). A crash in the tiny window between the link commit and the delivery-row insert loses that event. This is far smaller than the current in-memory gap; a fully same-transaction enqueue is a documented follow-up.

## Schema
`webhook_deliveries` — one row per (event, subscription) delivery attempt-set:
```
id            BIGSERIAL PRIMARY KEY
delivery_key  TEXT UNIQUE NOT NULL   -- stable idempotency id = "<event_id>.<subscription_id>"
subscription_id BIGINT NOT NULL
event_type    TEXT NOT NULL
payload       TEXT NOT NULL          -- the signed/unsigned body already built by webhook_event_payload
created       BIGINT NOT NULL
attempts      INT NOT NULL DEFAULT 0
next_attempt_at BIGINT NOT NULL      -- when the relay may next try (now on insert)
delivered_at  BIGINT                 -- NULL until delivered
dead          BOOLEAN NOT NULL DEFAULT FALSE  -- DLQ flag after MAX attempts
```
Index: `(dead, delivered_at, next_attempt_at)` for the relay poll. `delivery_key` UNIQUE gives insert-time idempotency (a duplicate enqueue is a no-op via `ON CONFLICT (delivery_key) DO NOTHING`).

## Store methods (Postgres; LMDB may `unimplemented!()`/no-op since outbox is Postgres-only — but keep the trait honest, see below)
Add to the `Store` trait (all async):
- `enqueue_deliveries(&self, rows: &[OutboxRow]) -> Result<(), StoreError>` — bulk `INSERT ... ON CONFLICT (delivery_key) DO NOTHING`.
- `claim_due_deliveries(&self, now: u64, limit: i64) -> Result<Vec<OutboxDelivery>, StoreError>` — `SELECT ... WHERE dead=false AND delivered_at IS NULL AND next_attempt_at <= $now ORDER BY next_attempt_at FOR UPDATE SKIP LOCKED LIMIT $limit` inside a short tx that the caller commits after marking (or use `FOR UPDATE SKIP LOCKED` + immediate status update in the same tx). Two relays never claim the same row.
- `mark_delivered(&self, id) ` / `mark_retry(&self, id, next_attempt_at, attempts)` / `mark_dead(&self, id)`.
For the LMDB backend these can be no-ops that return empty/Ok, because `main.rs` never spawns the relay on LMDB and never routes lifecycle events to the outbox on LMDB. Document that.

`OutboxRow { delivery_key, subscription_id, event_type, payload, created, next_attempt_at }`, `OutboxDelivery { id, delivery_key, subscription_id, event_type, payload, attempts }`.

## Emit path (api.rs, Postgres backend)
When emitting a lifecycle event on the Postgres backend, instead of `dispatcher.emit(...)`:
1. Read the active subscriptions matching the event type (reuse the dispatcher's snapshot or `store.list_webhooks`).
2. Build one `OutboxRow` per matching subscription: `delivery_key = "<event_id>.<sub_id>"` (event_id from the payload), `payload` = the body from `webhook_event_payload`.
3. `store.enqueue_deliveries(&rows)` (best-effort: log on error, do not fail the admin request; but this is a durable insert, so an error is rare).
The routing (outbox vs in-memory channel) is decided once, at wiring time, by backend. Keep `link.clicked` on `dispatcher.emit`.

## Relay worker (new, Postgres-only)
`spawn_webhook_relay(store, client)` — mirrors the analytics/webhook worker shape, spawned in `main.rs` ONLY on the Postgres backend:
- Loop on a short interval (e.g. 1s): `claim_due_deliveries(now, BATCH)`.
- For each claimed delivery: look up the subscription (for url/secret/kind — cache a snapshot refreshed on a ticker like the current worker), SSRF-guard the destination (reuse `is_internal_host`/`extract_host`), build the outgoing request (reuse `build_outgoing_request` so Generic signs per Standard Webhooks and channel kinds format their payload), set the `webhook-id` header to the persisted `delivery_key` (stable across attempts and nodes — the idempotency win), POST with the existing timeout/redirect(none) client.
- On 2xx: `mark_delivered`. On failure: `attempts+1`; if `attempts >= MAX_ATTEMPTS` → `mark_dead` (DLQ); else `mark_retry(next_attempt_at = now + backoff(attempts))` with exponential backoff + jitter spanning up to minutes (persisted, survives restart).
- If the subscription was deleted since enqueue: `mark_dead` (nothing to deliver to) or `mark_delivered` with a note; pick one and document.
- Per-delivery isolation: because each row is independent and claimed via SKIP LOCKED, a slow/broken endpoint does not head-of-line-block other subscriptions (unlike the current single serial worker). Optionally cap concurrency.

Reuse from `src/webhooks/delivery.rs`: `build_outgoing_request`, `sign`, `matches`, the reqwest client config (timeout, `redirect(none)`), `DELIVERY_ATTEMPTS`/backoff constants (rename/extend as needed for the persisted schedule).

## Wiring (main.rs)
- Detect backend (Postgres vs LMDB) as already done. On Postgres: spawn `spawn_webhook_relay`; route lifecycle emits to the outbox. On LMDB: unchanged (in-memory worker + channel for everything).
- Keep the existing `spawn_webhook_worker` for the in-memory path (LMDB, and clicked on Postgres).

## Testing (gated on QUARK_TEST_DATABASE_URL = postgres://quark:quark@127.0.0.1:5432/quark)
`tests/webhook_outbox_it.rs`:
- enqueue N deliveries → relay (pointed at a local mock HTTP server via the injectable base/seam) delivers all, rows marked delivered.
- a failing (500) endpoint → row goes through attempts with growing next_attempt_at, then `dead=true` after MAX (DLQ), and stops being claimed.
- SKIP LOCKED: two concurrent `claim_due_deliveries` calls return disjoint rows (no double delivery).
- idempotency: the `webhook-id` header equals `delivery_key` and is identical across two attempts of the same row.
- `ON CONFLICT (delivery_key) DO NOTHING`: enqueuing the same (event, sub) twice inserts one row.
Also keep the existing in-memory delivery tests green (LMDB path unchanged).

## Docs
`docs/WEBHOOKS.md` + `.PT_BR.md`: a "Durable delivery (Postgres)" section — the outbox + leased relay, at-least-once with persisted retry + dead-letter, the `webhook-id` = delivery_key idempotency (receivers dedup on it), that lifecycle events are durable while `link.clicked` is best-effort by design, and the residual same-tx follow-up. avoid-ai-writing (no em-dashes/AI-isms), pt-BR natural twin.

## Global constraints
Code English; no inline `//`; the redirect hot path is untouched (no synchronous DB write added to it); LMDB/single-node behavior unchanged; `QUARK_ADMIN_TOKEN` unchanged; SSRF guard on every delivery destination; docs EN + PT_BR; no merge to main; Rust tests `-j1`; Postgres tests gated. StubStore mock in delivery.rs updated for any new trait methods.
