# Bot/crawler filter for analytics — design + plan (roadmap #5)

**Date:** 2026-07-14
**Branch:** `feat/bot-filter` (stacked on `feat/richer-analytics` / #3; no merge until reviewed). Reviewing/merging this branch also brings #3.
**Effort:** low-medium. Extends the analytics module + ClickHouse + stats UI.

## Goal

Detect likely bot/crawler clicks (from the User-Agent) and exclude them from the
stats breakdowns, while showing how many were flagged. Matches the
precision/privacy angle; few OSS shorteners do it well.

## Decisions (locked, user delegated)

- **`is_bot(ua: Option<&str>) -> bool`** heuristic, dependency-free (same style as
  `device_from_ua`/`os_from_ua` added in #3). Flags common crawler/library UAs:
  substrings like `bot`, `crawler`, `spider`, `crawl`, `slurp`, `bingpreview`,
  `facebookexternalhit`, `embedly`, `curl`, `wget`, `python-requests`, `httpie`,
  `go-http-client`, `axios`, `headless`, `phantomjs`, `preview`, `monitor`,
  `uptime`, `pingdom`. Empty/absent UA → treated as bot (no real browser sends
  no UA). Case-insensitive.
- **Aggregates:** add `bots: u64` (count of clicks flagged as bot). The existing
  breakdowns (`per_day/country/device/os/browser/referer/city`) and NOT `total`
  are computed over **human** clicks only (skip when `is_bot`). `total` stays the
  count of all clicks; human total is `total - bots` (derived). This makes bots
  "excludable" from the analysis breakdowns while keeping the raw count honest.
- **CRITICAL (lesson from #3):** `Aggregates` is a persisted incremental JSON
  blob (LMDB + Postgres). The new `bots` field MUST carry `#[serde(default)]`,
  and a regression test must deserialize a pre-#5 blob (without `bots`) and get
  `bots = 0`. Without this, every link with an existing aggregate breaks.
- **Recent events flag:** `ClickEvent` gains `bot: bool` with `#[serde(default)]`,
  set when the stats builder assembles the recent-events list
  (`ev.bot = is_bot(ev.user_agent)`), uniformly for both backends. It is a
  response-side flag; storage may leave it default. The UI badges bot rows.
- **ClickHouse:** add a `bot UInt8` column computed at write via `is_bot`, with
  `ALTER TABLE ... ADD COLUMN IF NOT EXISTS` migration (like #3's os/browser).
  Aggregate queries add `WHERE bot = 0` for the human breakdowns; add
  `countIf(bot = 1) AS bots`. Recent-events selection derives the flag.

## Tasks

### Task 1 — analytics core: `is_bot` + `bots` count + human-only breakdowns + recent flag
Files: `src/analytics/mod.rs`.
- `is_bot`; `Aggregates.bots: u64` `#[serde(default)]`; `apply()` increments
  `bots` and skips the per_* breakdowns when the event is a bot. Recent-events
  builder sets `ClickEvent.bot`. `ClickEvent.bot: bool` `#[serde(default)]`.
- Tests: `is_bot` vectors (Googlebot, curl, empty→bot, a real Chrome UA→not bot);
  `apply()` with a mix → `bots` counted and per_country excludes the bot;
  **regression: deserialize an Aggregates blob without `bots` → `bots == 0`**.

### Task 2 — ClickHouse: `bot` column + migration + human-only queries + bots count
Files: `src/analytics/clickhouse.rs`.
- `bot UInt8` in `CREATE TABLE`; `ALTER TABLE clicks ADD COLUMN IF NOT EXISTS
  bot UInt8` migration; write computes `is_bot` → 0/1. `stats()`: per_* queries
  get `WHERE bot = 0`; add `countIf(bot = 1)` for `bots`; recent select maps the
  bot flag. Gated test (ClickHouse) mirrors #3's: bot clicks counted in `bots`
  and excluded from per_country/etc.

### Task 3 — stats UI + docs
Files: `web/src/routes/LinkStats.tsx` (a "Bots (excluded)" StatCard),
`web/src/components/RecentEventsTable.tsx` (a bot badge on flagged rows),
`web/src/lib/types.ts` (`bots`, `ClickEvent.bot`), i18n `en.ts`/`pt-BR.ts`;
`docs/ANALYTICS.md`/`.PT_BR.md` (a "Bot filtering" section: what is flagged,
that breakdowns are human-only, the honest total). Vitest updated. No em-dashes.

## Global constraints

- `bots` (and `ClickEvent.bot`) are new fields on PERSISTED structs → both need
  `#[serde(default)]` + a deserialize-old-blob regression test.
- Human breakdowns exclude bots; `total` stays all clicks; `bots` shown.
- Empty UA counts as bot. Heuristic only (documented as "potential" bots).
- No new runtime dependency; hot path unchanged (bot-ness computed at aggregate
  time / write time, not on the redirect path beyond the UA already captured).
- All code English; UI via i18n (EN + PT-BR); docs EN + `PT_BR`, no em-dashes.
- Rust tests `-j1`; ClickHouse gated test skips cleanly when absent.
- Stacked on #3; stay on `feat/bot-filter`; do not merge to main.

## Out of scope

- IP/ASN-based bot detection (needs external data); UA heuristic only.
- A per-subscription "include bots" toggle in the UI (breakdowns are human-only;
  a toggle is a possible follow-up).
- Blocking bots from redirecting (this only filters analytics; redirects still
  serve everyone).
