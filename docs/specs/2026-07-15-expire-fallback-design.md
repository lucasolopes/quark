# Expire-with-fallback — design + plan

**Date:** 2026-07-15
**Branch:** `feat/expire-fallback` (off main; no merge until reviewed)
**Effort:** small. Adds one optional field to the link `Record`
(`fallback_url`), a redirect-path branch, validation, and UI. Same shape as the
`folder`/`app_ios` field additions.

## Goal

When a link expires, redirect to a configured fallback URL instead of returning
`410 Gone`. A marketer whose campaign link expired can send late clicks to a
"this offer ended, see what's new" page instead of a dead end. When no fallback
is set, behavior is unchanged (`410 Gone`).

## Scope decision

- The fallback fires for **both** expiration triggers: time expiry
  (`now >= rec.expiry`) and visit-count exhaustion (`n > rec.max_visits`). In
  both states the link is "used up", so both deserve the fallback. (User
  delegated the choice; both chosen for consistency.)
- **Plain redirect, no interstitial.** The fallback is a `302` straight to the
  configured URL. No confirmation page — that is the separate password/interstitial
  feature.
- Single fallback URL per link (an `Option<String>`), not a list or rules. YAGNI.

## Decisions (locked)

- `Record.fallback_url: Option<String>` with
  `#[serde(default, skip_serializing_if = "Option::is_none")]` — mirrors
  `app_ios`. Old persisted blobs deserialize to `None`; the field is omitted when
  absent. LMDB stores the whole Record as JSON, so serde default covers it.
- **Postgres:** `fallback_url TEXT` column on `links`, added via
  `ALTER TABLE links ADD COLUMN IF NOT EXISTS fallback_url TEXT` (non-destructive,
  same as `folder`). Read/written at every Record site (`put_link`,
  `put_alias_and_link`, `get_link`, `list_links`, `search_links`, `row_to_link`).
- **Redirect hot path (`src/api.rs::redirect`):** at each of the two existing
  `410 Gone` returns (time branch ~api.rs:776, visit branch ~api.rs:790), first
  check `rec.fallback_url`:
  - `Some(url)` → `302 Found` with `Location: url` and `Cache-Control: no-store`.
  - `None` → `410 Gone` exactly as today.
  - The common (non-expired) path is untouched: `fallback_url` is only read
    inside branches that were already returning 410. **Zero added cost** for
    live links.
  - The existing `link.expired` webhook emit (time branch, gated on
    `expired_subscribed`) stays and still fires **before** the response is built,
    for both the 302 and 410 outcomes.
  - `Cache-Control: no-store` on the fallback 302: visit-count expiry is
    per-request, so the response must not be cached.
- **Validation (`src/api.rs`, create `POST /` and `admin_link_patch`):** an
  optional `fallback_url` field. When present and non-empty it must parse as
  `http`/`https` and be non-internal (`abuse::is_internal_host`), same guard as
  the primary `url` and other destinations. Empty string / absent → `None` (no
  fallback). Reject invalid with `400`, same error shape as an invalid primary URL.
- `LinkRow` (panel API response) gains `fallback_url: Option<String>` so the edit
  dialog can prefill it.

## Non-goals

- Interstitial/confirmation page (separate feature).
- Per-rule or multi-URL fallbacks.
- A distinct "expired vs visit-exhausted" fallback — one URL covers both.

## Tasks

### Task 1 — backend: `Record.fallback_url` + all store sites

**Files:** `src/store/mod.rs` (Record field), `src/store/lmdb.rs`,
`src/store/postgres.rs` (migration + all read/write sites), tests.

- Add `fallback_url: Option<String>` to `Record` with
  `#[serde(default, skip_serializing_if = "Option::is_none")]`.
- Postgres: `ADD COLUMN IF NOT EXISTS fallback_url TEXT` in the migration block;
  thread the column through `put_link`, `put_alias_and_link`, `get_link`,
  `list_links`, `search_links`, `row_to_link`.
- Tests:
  - Record round-trips `fallback_url` (LMDB unit; Postgres gated by
    `QUARK_TEST_DATABASE_URL`).
  - **Regression:** an old Record JSON blob without `fallback_url` deserializes
    to `None` (not an error).

### Task 2 — redirect: 302-to-fallback on expiry

**Files:** `src/api.rs` (the `redirect` handler, both 410 branches), tests.

- Extract a small helper, e.g.
  `fn expired_response(fallback: Option<&str>) -> Response`, returning the
  fallback `302` (with `no-store`) when `Some`, else the current `410 Gone`
  (with `no-store`). Both 410 sites call it so the logic is not duplicated.
- Tests (axum handler / integration level, following existing redirect tests):
  - Time-expired link **with** `fallback_url` → `302` + correct `Location` +
    `Cache-Control: no-store`.
  - Time-expired link **without** `fallback_url` → `410` (unchanged).
  - Visit-exhausted link **with** `fallback_url` → `302` to fallback.
  - Visit-exhausted link **without** `fallback_url` → `410` (unchanged).
  - Live (non-expired) link → normal `302` to destination, `fallback_url`
    irrelevant.

### Task 3 — API: accept + validate + expose `fallback_url`

**Files:** `src/api.rs` (create body, patch body, `LinkRow`), tests.

- `POST /` and `PATCH /admin/links/:code` accept optional `fallback_url`.
- Validate when non-empty: `http`/`https` + `!is_internal_host`; else `400`.
  Empty/absent → `None`.
- `LinkRow` gains `fallback_url`.
- Tests: create with valid fallback → stored and returned in `LinkRow`; create
  with `javascript:`/internal-host fallback → `400`; patch clears it with empty
  string.

### Task 4 — frontend: fallback URL field in the link dialog

**Files:** `web/src/lib/types.ts`, `web/src/lib/api.ts`, the create/edit link
dialog component, `web/src/i18n/en.ts` + `pt-BR.ts`, Vitest.

- Add `fallbackUrl` to the link type and to the create/patch payloads.
- Add an optional "Fallback URL (on expiry)" input to the dialog, prefilled from
  `LinkRow.fallback_url`. Placeholder + helper text explaining it redirects here
  instead of showing an expired page. i18n EN + PT-BR.
- Test: dialog renders the field; submitting sends `fallbackUrl`.

### Task 5 — docs

**Files:** `docs/` redirect/link behavior doc, README note if links are
documented there; both EN + PT_BR where the repo keeps bilingual docs.

- Document that an expired link with a fallback URL returns `302` to the fallback
  instead of `410`, for both time and visit-count expiry.

## Global constraints

- Hot path pays **zero** extra cost for non-expired links (field read only inside
  the already-410 branches).
- SSRF guard (`is_internal_host`) on the fallback destination, like every other
  URL the service will redirect to.
- Code in English; UI i18n EN + PT-BR; docs EN + PT_BR.
- Non-destructive Postgres migration (`ADD COLUMN IF NOT EXISTS`).
- Old persisted Records without the field must keep working (serde default).
- `-j1` / `CARGO_BUILD_JOBS=1` for Rust builds/tests in this environment; kill
  `quark.exe` before building.
