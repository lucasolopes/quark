# Richer click analytics — design + plan (roadmap #3)

**Date:** 2026-07-14
**Branch:** `feat/richer-analytics` (off main; no merge until reviewed)
**Effort:** medium. Touches the analytics module, the ClickHouse sink, click
capture, and the stats UI. No new runtime dependency (heuristic UA parsing,
header-based geo).

## What exists

`ClickEvent { id, ts, referer, country, user_agent }` captured from headers
(`cf-ipcountry`, `referer`, `user-agent`). `Aggregates` computes `per_day`,
`per_country`, `per_device` (coarse `device_from_ua` → Mobile/Desktop/Other).
LMDB computes aggregates in Rust from stored raw events; ClickHouse stores
`country, device, referer` columns (device computed at write) and aggregates by
`GROUP BY`. Stats UI (`StatsCharts.tsx`) shows per-day, per-country, per-device.
No raw IP is ever stored.

## Net-new (this feature)

- **OS** and **browser** breakdowns (heuristic, dependency-free parsers).
- **Referrer** breakdown (grouped by host).
- **City** geo, via an optional proxy header (`cf-ipcity`), no GeoIP database.
- **Privacy posture** documented: no IP retained; only coarse aggregates + a
  capped ring of recent events; geo/city come from the edge proxy header, not
  from IP lookup in quark.

## Decisions (locked, user delegated)

- UA parsing stays dependency-free heuristics (same style as `device_from_ua`):
  `os_from_ua` → Windows/macOS/iOS/Android/Linux/Other; `browser_from_ua` →
  Chrome/Safari/Firefox/Edge/Other (order matters: Edge before Chrome, Chrome
  before Safari).
- Referrer grouped by **host** (`referer_host(referer)` → the hostname, or
  "direct" when absent). Avoids unbounded cardinality from full URLs.
- City is best-effort from `cf-ipcity`; usually empty (most setups don't send
  it), so the UI hides an empty breakdown. No GeoIP DB (keeps the zero-dep core).
- Aggregates for OS/browser/referrer on LMDB are derived from the already-stored
  `user_agent`/`referer` (no storage migration). ClickHouse needs `os`,
  `browser`, `city` columns (computed at write like `device`) plus `GROUP BY`
  queries; migrate with `ALTER TABLE clicks ADD COLUMN IF NOT EXISTS`.

## Data model changes

- `ClickEvent` gains `city: Option<String>`.
- `Aggregates` gains `per_os: BTreeMap<String,u64>`, `per_browser: ...`,
  `per_referer: ...`, `per_city: ...`; `apply` fills them.
- ClickHouse `clicks` table gains `os String, browser String, city String`
  (backfilled empty on existing rows → bucketed "Unknown"/"Other").

## Tasks

### Task 1 — analytics core: parsers + aggregates + ClickEvent.city (LMDB path)
Files: `src/analytics/mod.rs`, `src/api.rs` (capture `cf-ipcity`).
- Add `os_from_ua`, `browser_from_ua`, `referer_host` (all `&str`/String, unit
  tested with representative UAs/referers), and `ClickEvent.city`.
- Extend `Aggregates` with `per_os/per_browser/per_referer/per_city` and fill in
  `apply`. Capture `city` from `cf-ipcity` in the redirect handler next to
  `country`.
- Tests: `os_from_ua`/`browser_from_ua`/`referer_host` vectors; `Aggregates::apply`
  populates the new maps from a set of events. `cargo test --lib -j1`.

### Task 2 — ClickHouse sink: os/browser/city columns + grouped queries
Files: `src/analytics/clickhouse.rs`.
- `CREATE TABLE` includes the new columns; add `ALTER TABLE ... ADD COLUMN IF NOT
  EXISTS` for existing tables (migration). Write path computes os/browser via the
  new parsers and passes city.
- `stats()` adds `GROUP BY` for per_os, per_browser, per_city, and per_referer
  (group by referer host — either store a `referer_host` or `domain(referer)` in
  SQL). Gated test (`QUARK_TEST_DATABASE_URL`? ClickHouse gate) mirrors the
  existing analytics IT pattern; skips cleanly when the service is absent.

### Task 3 — stats UI: OS/browser/referrer/city charts + i18n + privacy doc
Files: `web/src/components/StatsCharts.tsx`, `web/src/lib/types.ts`,
`web/src/i18n/{en,pt-BR}.ts`; `docs/ANALYTICS.md` + `docs/ANALYTICS.PT_BR.md`.
- Add charts for per-os, per-browser, per-referer (top N), and per-city (only
  when non-empty), mirroring the existing per-country/per-device chart. i18n
  keys EN + PT-BR. Vitest updated.
- `docs/ANALYTICS.md` (+ PT_BR): what is captured, the privacy posture (no IP
  stored, header-based geo, coarse aggregates, capped recent ring, how to turn
  the edge geo headers on), language-nav header, no em-dashes.

## Global constraints

- No raw IP stored, ever (verify in capture). No new runtime dependency.
- Redirect hot path unchanged in cost: capture already builds `ClickEvent`
  unconditionally for analytics; adding `city` is one more header read, no new
  synchronous work.
- All code English; UI via i18n (EN + PT-BR); docs EN + `PT_BR`, no em-dashes.
- Aggregates additive: LMDB derives new breakdowns from stored raw events (no
  migration); ClickHouse migrates with `ADD COLUMN IF NOT EXISTS`.
- Rust tests `-j1`; ClickHouse/Postgres gated tests skip cleanly when absent.
- Stay on `feat/richer-analytics`; do not merge to main.

## Out of scope

- GeoIP database / IP-to-city lookup (would add a heavy dep + data file; the
  header path covers city for edge deploys).
- Bot/crawler filtering (that is roadmap #5, its own feature).
- Full per-URL referrer detail (host grouping is enough; raw referrer stays in
  the recent-events ring).
