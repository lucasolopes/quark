# Pixel dedup keys — design (scale-audit #2)

**Branch:** `feat/scale-hardening` (stacked on #1). NOT merged.
**Closes:** scale-audit gap #3 — pixel forwarding carries no dedup key, so any future retry/outbox would double-count conversions at GA4/Meta.

## Problem

`meta_payload` and `ga4_payload` (`src/pixel.rs`) emit no per-click identifier. `ClickEvent` has no unique click id (only the link `id` + `ts`). Without a stable dedup key, adding at-least-once delivery / retries later (the durability we want) would over-report conversions with no way for the provider to collapse the duplicates. This is the cheap prerequisite that must land BEFORE any pixel retry/outbox.

## Design

### Stable per-click id on `ClickEvent`
Add `pub event_id: String` to `ClickEvent`, generated ONCE at click capture in the redirect handler (`src/api.rs`, where `let ev = ClickEvent { ... }` is built, ~line 844). Generation: 16 random bytes hex, prefixed `clk_` (mirror `webhooks::generate_event_id` / `generate_msg_id`). Because it is generated once and carried through the in-memory channel to the worker, it is stable for the life of that click — the same value would be sent on every retry of the same conversion.

Persistence: `#[serde(default)]` so old recent-events blobs deserialize (empty string). It DOES persist in the recent buffer (unlike ip/fbc), because a durable, replay-safe id is exactly what #4 (idempotent sink writes) will reuse. A short id in the capped recent buffer is acceptable.

### Meta CAPI — real dedup win
In `meta_payload`, add `"event_id": e.event_id` at the event level (sibling of `event_name`/`event_time`). This is the field Meta uses to deduplicate a conversion across retries and against the browser Pixel (https://developers.facebook.com/docs/marketing-api/conversions-api/deduplicate-pixel-and-server-events). With this, an at-least-once retry is safe: Meta collapses duplicates by `event_id`.

### GA4 MP — include the key, document the limit honestly
In `ga4_payload`, add `"transaction_id": e.event_id` to each event's `params`. IMPORTANT caveat to document: GA4 Measurement Protocol only deduplicates `purchase`-type events by `transaction_id`; it does NOT dedup an arbitrary custom event like `quark_click`. So for GA4 the id is included for completeness and operator-side reconciliation, but GA4 retries can still double-count. The doc must state this plainly so no one assumes GA4 dedup they do not have. The Meta path is the one that is genuinely retry-safe.

### No behavior change to what is already sent
All existing fields (link_code, country, user_data, client_id) stay identical. Only the id fields are added.

## Files
- `src/analytics/mod.rs` — add `event_id` to `ClickEvent` (+ serde default) and any test constructors; update `ev()`-style test helpers.
- `src/api.rs` — generate `event_id` when building the redirect `ClickEvent`. A small helper `fn generate_click_id() -> String` (or reuse an existing generator).
- `src/pixel.rs` — `meta_payload` adds `event_id`; `ga4_payload` adds `transaction_id` param; update the payload-shape unit tests to assert both.
- `docs/CONVERSION-FORWARDING.md` + `.PT_BR.md` — document the dedup keys and the GA4-vs-Meta dedup difference; note this makes future at-least-once delivery safe for Meta.

## Testing
- Unit: `meta_payload` output contains `event_id` == the event's id; `ga4_payload` params contain `transaction_id`; the same `ClickEvent` always yields the same id in the payload (stability). ClickEvent old-blob deserialization still works (event_id defaults to empty).
- Every `ClickEvent { ... }` literal across src/tests updated to set `event_id`.

## Global constraints
Code English; no inline `//`; `#[serde(default)]` on the new field + old-blob regression; docs EN + PT_BR, avoid-ai-writing; no merge to main; Rust tests `-j1`.
