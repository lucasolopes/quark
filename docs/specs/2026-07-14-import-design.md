# Migration importer — design + plan (roadmap #4)

**Date:** 2026-07-14
**Branch:** `feat/import` (off main; no merge until reviewed)
**Effort:** low-medium. Backend bulk-import endpoint + a small refactor + panel UI + docs.

## Goal

Let an operator migrate existing links into quark from a Bitly / Kutt / YOURLS
export (CSV or JSON), or any generic list. A wedge to steal users from
competitors. Bulk-create links, reporting per-row success/failure.

## What exists

`POST /` (`create` handler in `src/api.rs:90-171`) validates the URL
(`is_valid_url`, `extract_host`, blocklist/anti-loop guards), computes expiry
from `ttl`, then either `put_alias_and_link` (custom alias) or `next_id` +
`put_link` (numeric code). Admin token gates it when set.

## Decisions (locked, user delegated)

- **Refactor first (DRY):** extract `create_link_core(st, url, alias, ttl,
  headers) -> Result<String, CreateError>` from the `create` handler. The
  handler keeps admin/rate-limit checks then calls the core and maps
  `CreateError` to HTTP status. Import loops rows through the same core, so
  validation and blocklist rules are identical. Behavior-preserving; the
  existing `create`/`POST /` tests are the safety net.
- **Endpoint:** `POST /admin/import` (admin token, always required regardless of
  `QUARK_ADMIN_TOKEN`, since it is an admin-only bulk op). Body is CSV or JSON.
- **Formats:**
  - JSON: an array of `{url, alias?, ttl?}` (also accept `long_url`/`longUrl`
    and `keyword`/`short` as aliases for url/alias via serde alias).
  - CSV: header row; auto-detect the URL column (`url`, `long_url`, `longUrl`,
    `original_url`, `long`) and the alias column (`alias`, `keyword`, `short`,
    `short_code`, `custom`); optional `ttl`/`expiry` (seconds). Use the `csv`
    crate (small, correct quoting) rather than hand-rolled splitting.
  - Format chosen by `Content-Type` (`application/json` vs `text/csv`) with a
    fallback sniff (leading `[` or `{` → JSON).
- **Cap:** at most `MAX_IMPORT_ROWS = 10_000` rows per request (`413`/`400` over
  the cap) to bound memory and runtime.
- **Result:** `200 { "imported": N, "failed": [{ "index": i, "url": "...",
  "reason": "invalid url" | "alias in use" | "blocked destination" | ... }] }`.
  Partial success is fine: good rows import, bad rows are reported, the request
  never aborts on the first bad row.
- Reuse `is_internal_host`/blocklist guards via the core. No new hot-path
  surface (import is admin-only, not the redirect path).

## Components / tasks

### Task 1 — backend: extract `create_link_core` + `POST /admin/import`
Files: `src/api.rs` (+ maybe `src/import.rs` for the parsing), `Cargo.toml`
(`csv`), tests in `tests/`.
- `enum CreateError { InvalidUrl, NoHost, Blocked, AliasCollision, AliasInUse,
  InvalidTtl, IdExhausted, Backend }`; `create_link_core(...) -> Result<String,
  CreateError>` holding the current create body (minus admin/rate-limit).
  Rewrite `create` to call it (map error→status exactly as today).
- `import_rows(body, format) -> Vec<ImportRow>` parsing JSON/CSV into
  `{url, alias: Option, ttl: Option}` with the column/field aliases above.
- `admin_import` handler: admin_guard, parse, cap check, loop core, build
  summary. Emits `link.created` webhook events? No (keep #4 independent of the
  webhooks branch; this branch is off main). Just create + summarize.
- Tests (`tests/import_it.rs`): JSON import creates N links (verify resolvable);
  CSV import with a YOURLS-style header (`keyword,url,...`) maps correctly;
  a bad URL row is reported in `failed` while good rows still import; an alias
  collision is reported; over-cap returns 400; `create`/`POST /` behavior
  unchanged (run existing `api_it`). Also a unit test that `create_link_core`
  and the old path agree (the existing create tests cover this).

### Task 2 — frontend: Import panel page
Files: `web/src/routes/Import.tsx`, `Shell.tsx` (nav), `router.tsx`, `api.ts`,
`queries.ts`, i18n `en.ts`/`pt-BR.ts`, `Import.test.tsx`.
- A page with a file picker (accept `.csv,.json`) and/or a textarea to paste,
  a "Import" button, and a results panel: "Imported N, M failed" with a table
  of failures (index, url, reason). Uses the existing api client + toasts.
- i18n EN + PT-BR. Vitest: paste JSON → calls API → shows summary; a failed row
  renders in the table.

### Task 3 — docs (EN + PT_BR) + README/ROADMAP
Files: `docs/IMPORT.md`, `docs/IMPORT.PT_BR.md`, README (both), ROADMAP (both).
- Formats and column mappings; concrete examples for exporting from Bitly,
  Kutt, YOURLS and importing; the partial-success/failure report; the cap.
  Language-nav header, no em-dashes.

## Global constraints

- `create_link_core` is behavior-preserving; existing `create`/`POST /` tests
  must stay green (they are the refactor's safety net).
- Import is admin-only; no redirect-hot-path impact.
- Partial success: never abort the whole import on one bad row.
- Cap at `MAX_IMPORT_ROWS = 10_000`.
- All code English; UI via i18n (EN + PT-BR); docs EN + `PT_BR`, no em-dashes.
- `csv` crate for CSV; serde for JSON. Rust tests `-j1`.
- Stay on `feat/import`; do not merge to main.

## Out of scope

- Importing click history/analytics from the source (only links).
- Preserving the source's original short codes as the quark code (a source
  code is kept only if provided as a custom `alias` and it passes the alias
  rules; otherwise quark assigns its own computed code).
- Async/background import for very large files (the 10k cap keeps it synchronous).
