# Server-side conversion forwarding — design + plan (roadmap #14)

**Date:** 2026-07-14 · **Branch:** feat/pixel-forwarding (off main; no merge) · **Effort:** medium-high.

## Goal
On each click, forward a server-side conversion event to GA4 (Measurement Protocol) and/or Meta (Conversions API), with no tracker on the client. A strong privacy differentiator: attribution without a client-side pixel.

## Scope (tight, to complete)
- **Instance-level** config (like the blocklist), not per-link, for this pass. One or more `PixelConfig`s; every click forwards to each active one. (Per-link targeting is a noted follow-up.)
- Providers: **GA4 Measurement Protocol** and **Meta Conversions API**. Others (GTM/TikTok/LinkedIn) are a follow-up once the pattern is proven.
- **Async only**: forwarding runs on the existing analytics worker path (batched clicks), NEVER on the redirect hot path. Fail-open (a provider error never affects redirects).
- **No SSRF surface**: provider hosts are fixed (`https://www.google-analytics.com`, `https://graph.facebook.com`); the operator supplies credentials, not URLs. The base host is injectable in code only for tests.

## Providers (payloads)
- **GA4**: `POST https://www.google-analytics.com/mp/collect?measurement_id=<id>&api_secret=<secret>` with `{client_id, events:[{name:"quark_click", params:{link_code, country}}]}`. A synthetic `client_id` derived per click (no real user id). GA4 MP accepts batches (up to 25 events) — batch the worker's flush.
- **Meta CAPI**: `POST https://graph.facebook.com/v19.0/<pixel_id>/events?access_token=<token>` with `{data:[{event_name:"Lead"|"ViewContent", event_time, action_source:"website", event_source_url, custom_data:{link_code}}]}`. Batch the `data` array.
- Privacy: forward only coarse fields already captured (country, referrer host, link code, timestamp). No IP, no raw UA sent unless the provider requires a hashed identifier (skip user-data hashing this pass; document that these are anonymous conversion pings).

## Tasks
### Task 1 — pixel config type + store + provider formatters + forwarder (pure/mockable)
Files: `src/pixel.rs` (new: `Provider{Ga4,MetaCapi}`, `PixelConfig{id,provider,credentials(a small struct/enum),active,created}`, `ga4_payload(events)`, `meta_payload(events)`, `forward(client, base, config, events)` — base host injectable), `src/lib.rs`, `Store` methods (list/get/put/delete/next_pixel_id) in LMDB (new db) + Postgres (table+migration), tests (formatter shapes vs known GA4/Meta structure; forward hits a mock server with the right path+body; store round-trip gated).
### Task 2 — wire into analytics worker + config endpoints + main
Files: `src/analytics/mod.rs` (the worker's flush also forwards the batch to active pixel configs via a `reqwest` client, fail-open), `src/api.rs` (`GET/POST/DELETE /admin/pixels`, Scope-free admin_guard as today), `src/main.rs` (client + configs available to the worker). Tests: creating a config then a click forwards to a mock provider; a down provider does not break the worker.
### Task 3 — UI + docs
Files: `web/src/routes/Pixels.tsx` (list/create/delete: provider select + credential fields; secrets shown masked), Shell nav, router, api/queries/types, i18n; `docs/CONVERSION-FORWARDING.md`+`.PT_BR.md`, README/ROADMAP. No em-dashes.

## Global constraints
- Forwarding is ASYNC (analytics worker), never on the redirect hot path; fail-open.
- No SSRF surface (fixed provider hosts; base injectable only for tests).
- Credentials stored; masked in GET; never logged.
- New persisted structs → serde(default) where added to existing structs; Postgres migration; regression. (PixelConfig is new, but any field added to ClickEvent/etc. needs the rule.)
- All code English; UI i18n EN+PT; docs EN+PT_BR, no em-dashes. Rust `-j1`; gated skips clean. Stay on feat/pixel-forwarding; no merge.

## Out of scope
- GTM/TikTok/LinkedIn providers (follow-up).
- Per-link targeting of pixels; user-data hashing / advanced matching; retry/durability beyond the worker's best-effort.
