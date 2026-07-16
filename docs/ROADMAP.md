**English** · [Português](ROADMAP.PT_BR.md)

# quark roadmap

## Current state

Essentially complete single-operator OSS product: **API + web panel**, under an
**AGPL-3.0 license** with a **CLA** for contributions. Single-binary,
zero-dependency core by default; network backends are opt-in via env vars
(chosen at startup, no build-time feature flag). Tested (57 lib tests + 29 API
tests + a gated Postgres/Valkey/ClickHouse integration suite, incl. search +
34 frontend tests) and benchmarked (permute ~264M ops/s; redirect ~7.9µs
in-process; in production it scaled linearly up to 1k VUs, with the measured
bottleneck being geography/RTT, not the server).

## Done

- **Webhooks (#1):** signed outgoing HTTP events on `link.created/updated/deleted/expired/clicked`,
  Standard Webhooks HMAC signing, SSRF-guarded. On Postgres the lifecycle events are delivered
  durably (a `webhook_deliveries` outbox + a leased relay with `FOR UPDATE SKIP LOCKED`, persisted
  retry/backoff, dead-letter after 8 attempts, and a stable idempotency key); `link.clicked` and
  `link.expired` stay best-effort in-memory by design (they fire on the redirect hot path). On LMDB
  every event rides the in-memory best-effort channel. Subscriptions managed in the panel or via
  `/admin/webhooks`. Foundation for #6 (Slack/Discord/Telegram) and #10 (n8n/Zapier). Doc: `docs/WEBHOOKS.md`.
- **Notification channels (#6):** Slack/Discord/Telegram as a `kind` on the webhook subscription
  (built on #1); an unsigned, plain-text message POSTed in each channel's own shape (Slack/Telegram
  `{"text": ...}`, Discord `{"content": ...}`), authenticated by the channel's secret URL instead
  of HMAC. Doc: `docs/WEBHOOKS.md` ("Notification channels").
- **Core (v0.1):** create + redirect + custom alias + expiration (TTL). The short code
  is a calibrated Feistel/ARX permutation (`ROUNDS=4`); codes are **computed, not
  stored** (store keyed by `u64`).
- **Max-visits expiration (#11):** a link expires by TTL or by a maximum number of visits, whichever comes first. An expired link can carry a `fallback_url` and redirect there (`302`) instead of returning `410`.
- **Password-protected links:** an opt-in per-link password (argon2id). A protected link serves a self-contained interstitial; the correct password sets a signed, per-code, 12h unlock cookie and the redirect proceeds. The plaintext is never stored; only `has_password` is exposed. Doc: `docs/LINK-PASSWORD.md`.
- **Broken-link monitoring:** an opt-in background checker (`QUARK_HEALTH_CHECK_SECS`) probes each destination, records its health, and emits `link.broken`/`link.recovered` webhooks on a transition. Panel shows a status dot and a "broken only" filter. Doc: `docs/LINK-HEALTH.md`.
- **OIDC login:** opt-in "sign in with your identity provider" (Authorization Code + PKCE), opaque revocable server-side sessions, default-closed claim-to-scope mapping. The `QUARK_ADMIN_TOKEN` stays a break-glass. Doc: `docs/OIDC-LOGIN.md`.
- **Google Sheets sync:** opt-in native OAuth connector (scope `drive.file`) that mirrors the link catalog into a spreadsheet the operator owns, synced on demand from the panel and on an optional lease-coordinated schedule. The refresh token is stored server-side and never returned. Doc: `docs/SHEETS.md`.
- **Pluggable architecture**: `Store` / `CacheTier` / `AnalyticsSink` traits:
  - **L2 Valkey** (`QUARK_VALKEY_URL`): shared cache, circuit breaker + timeout, fail-open.
  - **Postgres** (`QUARK_DATABASE_URL`): multi-node relational store (atomic id sequence).
  - **ClickHouse** (`QUARK_CLICKHOUSE_URL`): OLAP analytics sink (analytics-only).
- **Click analytics**: fire-and-forget capture on the 302 (~180ns) → worker → sink;
  `GET /:code/stats` (aggregates + last N events).
- **Observability**: opt-in per-request JSON access log (`QUARK_ACCESS_LOG`).
- **Edge/CDN**: TTL-aware `Cache-Control` on redirects (guide in `docs/EDGE.md`).
- **Horizontal scaling**: stateless replicas over a shared Postgres; `QUARK_NODE_ID`
  partitions the id space in LMDB (defensive guard). Doc: `docs/SCALING.md`.
- **Abuse protection** (only on `POST /`): per-IP rate limit (`QUARK_RATELIMIT_PER_MIN`,
  in-memory/Valkey, fail-open), built-in guard against internal/loop networks
  (`QUARK_BLOCK_PRIVATE`, on by default).
- **Panel API**: `GET /admin/links` (keyset-paginated list), `DELETE`/`PATCH /admin/links/:code`,
  all under `QUARK_ADMIN_TOKEN`. **Creating (`POST /`) requires the token when `QUARK_ADMIN_TOKEN`
  is set** (otherwise it stays public). Opt-in CORS via `QUARK_CORS_ORIGINS`.
- **Web panel (SPA)**: `web/` (React + Vite + shadcn/ui + TanStack + Recharts), deployed
  separately (static build), API-only binary. Token login → Links (CRUD, search,
  tags, copy, **QR code**) → Per-link stats (charts). UI/UX following
  Nielsen's heuristics.
- **Tags (#7)**: links carry normalized tags (`Record.tags`: trimmed, lowercased,
  deduped, capped) to organize them; the links list filters by tag
  (`GET /admin/links?tag=`), and `GET /admin/tags` lists the distinct set for the
  panel. A cross-tag aggregate stats dashboard stays a follow-up.
- **Server-side search (Postgres)**: `GET /admin/links?q=` runs `ILIKE` over url+alias
  (keyset-paginated, wildcards escaped). A Postgres-only feature; LMDB returns `501` and the
  panel falls back to **client-side** filtering (~300ms debounce, automatic fallback). A distinct
  error state from "nothing found".
- **License + contributions**: **AGPL-3.0-only** core; `CLA.md` (license grant) +
  `CONTRIBUTING.md` + a CLA bot (GitHub Action). Multi-tenancy/cloud stays proprietary, separate.
- **`docker-compose.yml`**: full stack (quark + Postgres + Valkey + ClickHouse) for dev/self-host.
- **Importer (#4)**: `POST /admin/import` bulk-creates links from a CSV or JSON export (Bitly, Kutt,
  YOURLS, generic), partial-success per-row report, plus a web panel "Import" tab. Doc:
  [`docs/IMPORT.md`](IMPORT.md).
- **UTM builder + templates**: collapsible UTM section in the create-link dialog, with a live
  destination preview and named templates saved locally (`localStorage`).
- **#9 API tokens with scopes + quota**: named tokens (`links_read`, `links_write`, `webhooks`, `analytics`, `full`) with an optional per-token rate limit, managed under `/admin/tokens` and the panel's **API tokens** page; the env `QUARK_ADMIN_TOKEN` keeps behaving as `full`, unchanged. Doc: `docs/API-TOKENS.md`.
- **Redirect rules (#12)**: per-link geo/device rules (first match wins, `url` stays the default), panel editor in the create/edit dialogs. Doc: `docs/REDIRECT-RULES.md`.
- **Conversion forwarding (#14)**: instance-level GA4/Meta CAPI pixels, forwarded async from the
  analytics worker (never the redirect hot path), fail-open. Panel: `/pixels`. Doc: `docs/CONVERSION-FORWARDING.md`.
- **A/B testing (#17)**: a link can carry weighted variants; redirects split traffic by a
  stateless weighted pick, with per-variant click stats. Doc: `docs/AB-TESTING.md`.
- **Deep linking (#20)**: hosts the iOS `apple-app-site-association` and Android
  `assetlinks.json` files at their well-known paths, editable in the panel (**App Links**), served
  as `application/json` over HTTPS with no redirect. The device-aware redirect ships too: a link can
  carry `app_ios` / `app_android` destinations, and a click from that platform (when the OS did not
  catch it) resolves to the app destination, ahead of geo/device rules and A/B variants in the
  precedence order. Guide: `docs/DEEP-LINKING.md`.
- **Scale hardening**: cross-node cache invalidation over a Valkey pub/sub channel
  (`src/invalidate.rs`); atomic Postgres analytics counters (`click_counters`, `INSERT ... ON
  CONFLICT`) plus append-only `click_events`, replacing the advisory-lock blob read-modify-write;
  a durable Postgres webhook outbox with a leased relay (retry/DLQ/idempotency); per-click Meta/GA4
  dedup ids; and a cluster preflight (`QUARK_STRICT_CLUSTER`) that fails fast when a strict multi-node
  deployment is missing Postgres or Valkey. Doc: `docs/SCALING.md`, audit: `docs/research/2026-07-14-scale-audit.md`.

## Next

- **Accounts + multi-user panel**: this is a **cloud-phase** feature (multi-tenant, proprietary). OSS
  stays single-account (single operator). Deserves its own brainstorming when the time comes.
- **Deploying the full version on the VPS**: the API + panel aren't in production yet
  (`quark.meuchat.ai` runs an older version); bring it up via Coolify.

## Backlog

For a wider set of candidate features, scored against short.io, Rebrandly, Bitly, and Dub,
see [`docs/research/2026-07-14-next-features.md`](research/2026-07-14-next-features.md).

- **Custom domains**: `mydomain.com/abc`.
- **Deep linking follow-ups**: deferred deep linking (send a user without the app to the store, then
  open it on the right screen after install) and in-app-browser routing (steering clicks out of an
  Instagram/TikTok webview). Both need a mobile SDK quark does not ship. The device-aware redirect
  itself is done (see Done).

## Design constraints (deliberate)

- **A pure binary (LMDB, no database) is single-node by design**: this is not a limitation to remove.
  Scaling means stateless replicas over a shared Postgres (`docs/SCALING.md`).
- **Abuse protection** runs only on `POST /`; the redirect (hot path) pays nothing for it.
- **Creating a link is public when there's no `QUARK_ADMIN_TOKEN`** (a zero-config open shortener);
  setting the token locks creation down to the operator.

## Parked (future, not planned)

- **Full-edge cloud on Cloudflare Workers**: the direction for the cloud edition (permute compiled to WASM;
  Store becomes KV/D1/Durable Objects). Parked until it becomes a priority.
- **Shared-nothing proxy** (multi-node LMDB without a database): not planned; Postgres already covers multi-node.

## Notes

- Anti-abuse, horizontal scaling, analytics, and the panel **have been delivered** (no longer future items).
- GitHub CI has a Rust job (with valkey+postgres+clickhouse services for the gated tests) and
  a `web` job (frontend lint/typecheck/test/build). The CLA is collected by a bot on every PR.
