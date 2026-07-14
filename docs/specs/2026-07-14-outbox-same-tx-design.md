# Transactional webhook outbox (close the dual-write window) — design

**Branch:** `fix/outbox-same-tx` (off `main@7ebc0e6`). NOT merged.
**Closes:** the residual dual-write window documented as a follow-up in the #3 outbox: on Postgres the delivery rows are enqueued right AFTER the link mutation commits, not in the same transaction. A crash in that window loses the event. This makes the enqueue atomic with the mutation.

## Current (the window)
In `src/api.rs`, four lifecycle emit sites do: `store.<mutation>` (commits) THEN `st.webhooks.emit_lifecycle(ev)` (a separate tx that reads subs + `enqueue_deliveries`). Sites:
- create alias path: `put_alias_and_link` then emit (api.rs ~428/435)
- create numeric path: `put_link` then emit (~459/464)
- delete: `delete_link` then emit (~1201/1210)
- patch: `put_link` then emit (~1345/1351)
`create_link_core` (shared by `POST /` and import) contains the create emits.

## Design — build rows outside the tx, enqueue inside it

### WebhookDispatcher: split "build" from "enqueue"
Add `pub async fn lifecycle_deliveries(&self, ev: WebhookEvent) -> Vec<OutboxRow>`:
- On the Postgres backend (`outbox` set): read matching active subs (`list_webhooks` + `matches`), build one `OutboxRow` per sub (`delivery_key = "<event_id>.<sub_id>"`, payload = ev.body), and RETURN them (do NOT enqueue).
- On LMDB (no outbox): call the existing in-memory `emit(ev)` and return an empty `Vec` (LMDB stays best-effort in-memory, unchanged).
Keep `emit_lifecycle` too (or reimplement it as `lifecycle_deliveries` + a fallback enqueue) so nothing else breaks, but the api.rs sites switch to the new two-step flow.

### Store: mutation + enqueue in one transaction
Add to the `Store` trait (async), Postgres real, LMDB delegating to the existing op and ignoring the (always-empty on LMDB) deliveries:
- `put_link_tx(&self, id: u64, rec: &Record, deliveries: &[OutboxRow]) -> Result<(), StoreError>` — one tx: upsert the link + `INSERT ... ON CONFLICT (delivery_key) DO NOTHING` the deliveries. Used by the create numeric path AND patch.
- `put_alias_and_link_tx(&self, alias: &str, id: u64, rec: &Record, deliveries: &[OutboxRow]) -> Result<bool, StoreError>` — one tx: claim the alias + put the link + enqueue; return `Ok(false)` WITHOUT enqueuing (tx rolls back) when the alias is already in use. So the enqueue is naturally conditional on the mutation succeeding.
- `delete_link_tx(&self, id: u64, deliveries: &[OutboxRow]) -> Result<(), StoreError>` — one tx: delete + enqueue.
On LMDB these are `self.put_link(...).await` / `self.put_alias_and_link(...).await` / `self.delete_link(...).await` (deliveries is empty, ignored). Update the `StubStore` mock in delivery.rs.

### Handler flow (api.rs) — Postgres path becomes atomic
Per site, restructure to: build the record + the event payload (needs the code, available after id allocation), build the delivery rows via `lifecycle_deliveries(ev)` (reads subs, OUTSIDE the tx), then call the `_tx` store method (mutation + enqueue in ONE tx). Concretely:
- create numeric: `id = next_id()`; `code = base62(encode(id,key))`; build the LinkCreated payload; `rows = lifecycle_deliveries(ev)`; `put_link_tx(id, &rec, &rows)`.
- create alias: `id = next_id()`; `canonical_code`; build payload; `rows = lifecycle_deliveries(ev)`; `put_alias_and_link_tx(alias, id, &rec, &rows)` returning the alias-claimed bool (unchanged semantics on false).
- delete: build the LinkDeleted payload for the code; `rows = lifecycle_deliveries(ev)`; `delete_link_tx(id, &rows)`.
- patch: build the LinkUpdated payload; `rows = lifecycle_deliveries(ev)`; `put_link_tx(id, &rec, &rows)`.
`create_link_core` holds the two create sites; keep it shared with import (import creates links, so import lifecycle events also become transactional). Preserve every existing behavior: the returned code, the alias-in-use 409, the MAX_ID check, the `link.clicked`/`link.expired` in-memory paths (untouched, still `emit`).

### What stays the same
- The redirect HOT PATH is untouched (no lifecycle enqueue there; clicked/expired stay in-memory `emit`).
- The relay, retry/DLQ, idempotency, SSRF, lease — all unchanged.
- LMDB single-node behavior byte-for-byte unchanged (empty deliveries, in-memory emit).
- Reading subs (`list_webhooks`) still happens per lifecycle event, outside the tx (a read, not part of the atomic write) — same cost as today.

## Testing (gated on QUARK_TEST_DATABASE_URL = postgres://quark:quark@127.0.0.1:5432/quark)
- `put_link_tx` / `put_alias_and_link_tx` / `delete_link_tx` insert the link AND the delivery rows in one tx: after the call, both the link and the `webhook_deliveries` rows are present; on an alias conflict, NEITHER the link nor the deliveries are written (rollback).
- Atomicity: enqueue with a delivery whose `delivery_key` already exists → `ON CONFLICT DO NOTHING`, the link still upserts, one delivery row.
- The existing webhook_outbox_it relay tests still pass (the rows are produced the same way, just via the tx now).
- api_it: create/patch/delete still return the same codes/statuses; webhooks_api_it still green.
- Keep the LMDB path green (StubStore/lmdb no-op deliveries).

## Global constraints
Code English; no inline `//`; redirect hot path untouched; LMDB/single-node unchanged; `QUARK_ADMIN_TOKEN` unchanged; SSRF unchanged; docs (WEBHOOKS + PT_BR, SCALING + PT_BR) updated to say lifecycle delivery enqueue is now same-transaction as the mutation (remove the residual-window follow-up note); avoid-ai-writing; no merge to main; Rust tests `-j1`; Postgres tests gated.
