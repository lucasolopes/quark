# Link tags — design + plan (roadmap #7)

**Date:** 2026-07-14
**Branch:** `feat/tags` (off main; no merge until reviewed)
**Effort:** low-medium. Adds a `tags` field to the link record (a persisted
struct → touches every Record read/write site), a list filter, and UI.

## Goal

Let the operator organize links with tags and filter the links list by tag
(the entry point to per-link stats). Parity feature.

## Scope decision

- Tags live ON the link `Record` (`tags: Vec<String>`), co-located with the
  link, not in a separate join table. Simpler model; the cost is touching every
  Record read/write site (mechanical).
- "Filter analytics by tag" is interpreted as **filtering the links list by
  tag** (`GET /admin/links?tag=...`), which is how you reach per-link stats. A
  cross-link aggregate-by-tag dashboard is OUT of scope (follow-up).

## Decisions (locked, user delegated)

- `Record.tags: Vec<String>` with `#[serde(default)]` (persisted; LMDB stores
  the whole Record as JSON so serde default covers old blobs). Normalize tags:
  trimmed, lowercased, deduped, non-empty, capped (e.g. 20 tags/link, each <=40
  chars) at the create/patch boundary.
- **Postgres:** a `tags JSONB NOT NULL DEFAULT '[]'` column on `links`, with
  `ALTER TABLE links ADD COLUMN IF NOT EXISTS tags JSONB NOT NULL DEFAULT '[]'`
  migration. Every `INSERT`/`SELECT` site (`put_link`, `put_alias_and_link`,
  `get_link`, `list_links`, `search_links`, `row_to_link`) reads/writes it.
- `create` (`POST /`) and `admin_link_patch` accept an optional `tags` array;
  normalize + store. `admin_links_list` accepts `?tag=<t>`: return only links
  whose `tags` contain `<t>` (Postgres `tags @> '["t"]'::jsonb`; LMDB filter in
  Rust). Combine with the existing `q=`/`after`/`limit` where reasonable
  (keyset still by id).
- `GET /admin/tags`: distinct tags across all links (for the UI filter). Bounded
  (LMDB scans the links; Postgres `SELECT DISTINCT jsonb_array_elements_text`).
- The `LinkRow` returned by the panel API gains `tags`.

## Tasks

### Task 1 — backend: `Record.tags` + all store sites + filter + `/admin/tags`
Files: `src/store/mod.rs` (Record), `src/store/lmdb.rs`, `src/store/postgres.rs`
(migration + all sites), `src/api.rs` (create/patch accept tags, `?tag=` filter,
`GET /admin/tags`, `LinkRow.tags`), tests.
- Normalization helper `normalize_tags(Vec<String>) -> Vec<String>`.
- Tests: Record round-trips tags (LMDB unit + Postgres gated); create with tags
  then list filtered by tag returns only matching; `GET /admin/tags` returns the
  distinct set; **regression: an old Record blob without `tags` deserializes to
  `[]`**; existing link tests stay green.

### Task 2 — frontend: tag chips + edit + filter
Files: `web/src/components/LinkTable.tsx` (tag chips per row),
`CreateLinkDialog.tsx`/`EditLinkDialog.tsx` (a tags input — comma-separated or
chip entry), `web/src/routes/Links.tsx` (a tag filter control driving `?tag=`),
`web/src/lib/{types,api,queries}.ts`, i18n `en.ts`/`pt-BR.ts`, tests.
- Tags shown as small badges on each link row. The create/edit dialogs let you
  set tags. A filter (dropdown from `GET /admin/tags`, or a text field) filters
  the list by tag via the query key. Vitest: setting tags on create sends them;
  the filter calls the API with `?tag=`.

### Task 3 — docs + ROADMAP
Files: README (both) short mention, `docs/ROADMAP.md`/`.PT_BR.md` mark #7.
(Tags are simple enough to document in the README panel section; no separate doc
file needed. No em-dashes.)

## Global constraints

- `Record.tags` is a new field on a PERSISTED struct → `#[serde(default)]` +
  deserialize-old-blob regression test + Postgres `ADD COLUMN IF NOT EXISTS`.
- Every Postgres Record read/write site updated (get/put/put_alias/list/search/
  row_to_link) — the crate must compile and existing link tests stay green.
- Tags normalized (trim/lowercase/dedupe/cap) at the API boundary.
- No redirect-hot-path change (tags are admin/list surface only; the redirect
  reads a link by id and does not need tags).
- All code English; UI via i18n (EN + PT-BR); docs EN + `PT_BR`, no em-dashes.
- Rust tests `-j1`; Postgres-gated tests skip cleanly when absent.
- Stay on `feat/tags`; do not merge to main.

## Out of scope

- Nested folders / hierarchy (flat tags only).
- Cross-link aggregate analytics per tag (a dashboard summing stats across a
  tag) — follow-up.
- Bulk tag-assignment UI (set tags per link via create/edit for now).
