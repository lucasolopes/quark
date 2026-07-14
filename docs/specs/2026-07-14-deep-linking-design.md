# Deep linking (foundation): app-association hosting — design

**Status:** approved (autonomous overnight; scoping decisions recorded below in lieu of live Q&A, under the standing "faça em ordem" authorization)
**Branch:** `feat/deep-linking` (off `main@1605510`) — NOT merged to main
**Roadmap:** item #20 (deep linking). This spec covers the low-risk, no-product-decision *core*; the interactive part is deferred (see Non-goals).

## Problem

For a short link to open a native mobile app instead of the browser (iOS
Universal Links, Android App Links), the domain that serves the redirect must
also serve two verification files the OS fetches directly:

- iOS: `apple-app-site-association` (AASA), a JSON document.
- Android: `assetlinks.json` (Digital Asset Links), a JSON document.

Without these hosted on the quark domain, no amount of redirect logic makes a
link open an app: the OS refuses to associate the domain with the app. So the
foundation of every deep-linking feature is *hosting these two files*. That is
what this brick delivers.

## Goals

1. Let an operator store the raw AASA and `assetlinks.json` documents (the
   exact files their mobile team produced) through the admin panel.
2. Serve them at the exact well-known paths the OSes fetch, with
   `Content-Type: application/json`, verbatim, over the normal HTTPS the
   redirect already uses, with no redirect and no auth (they are public by
   design).
3. Persist through the pluggable `Store` (LMDB default + Postgres), like every
   other config.

## Non-goals (deferred — needs your product decisions, do interactively)

- **Deferred deep linking / device-aware redirect** (detect iOS/Android and
  send to an app URI scheme or the store, with a web fallback). This overlaps
  the existing redirect-rules brick (#12) and requires product choices —
  which platforms, per-link app scheme, fallback behavior, whether it is a new
  rule field or a separate config. Left for an interactive session.
- Generating the documents from structured config (see Decision 1).
- Validating the *semantics* of the documents (app IDs, cert fingerprints).
  We validate that the body is JSON and within a size cap; correctness of the
  app association is the operator's mobile team's concern.

## Design decisions (recorded)

**Decision 1 — Store the raw JSON, serve verbatim (not build from a schema).**
Operators already have these two files from their mobile build (Xcode /
Android Studio emit them). Modelling them as structured quark config would
impose our schema, lag behind Apple/Google format changes (AASA has already
changed `paths` → `components`), and add code for no gain. We store the exact
bytes the operator pastes and serve them back. This is the "fazer mais com
menos" call: a two-key document store, not a document builder.

**Decision 2 — Validation is minimal and format-agnostic.** On write we check:
(a) the name is one of the two allowed document names; (b) the body parses as
JSON (`serde_json::from_str::<serde_json::Value>`); (c) the body is at most
64 KiB. We do not inspect the JSON shape. Invalid JSON is rejected with 400 so
an operator catches a paste error early; a valid-JSON-but-wrong-association is
served as-is (only the OS can judge it).

**Decision 3 — Public, unauthenticated GET; admin-guarded write.** The OS
fetches the files anonymously, so the GET routes carry no auth. Writing them
is under `QUARK_ADMIN_TOKEN` like every other admin endpoint.

**Decision 4 — Routing precedence.** Three GET routes are registered
explicitly, so axum's static-route matching wins over the `/:code` param
route:
- `/.well-known/apple-app-site-association`
- `/apple-app-site-association` (legacy root path; some iOS versions probe it)
- `/.well-known/assetlinks.json`
The `.well-known/*` paths are two-segment and never collided with `/:code`
(single segment) anyway; the legacy root AASA path is single-segment but a
static route outranks the dynamic one in axum, and no 7-char generated code
equals that string. An unset document returns 404 (not an empty JSON), which
is what the OS expects when a domain hosts no association.

**Decision 5 — Hot path untouched.** These are their own low-traffic routes,
resolved before any code lookup; the redirect hot path pays nothing. Each GET
is one cold `Store` read of a small string.

## Components

### `Store` trait (src/store/mod.rs)
Three async methods on the existing trait:
- `get_wellknown(&self, name: &str) -> Result<Option<String>, StoreError>`
- `put_wellknown(&self, name: &str, body: &str) -> Result<(), StoreError>`
- `delete_wellknown(&self, name: &str) -> Result<(), StoreError>`

**LMDB (src/store/lmdb.rs):** a new `Database<Str, Str>` named `wellknown`
(key = document name, value = raw JSON body). `MAX_DBS` bumps 6 → 7.

**Postgres (src/store/postgres.rs):** table
`wellknown_documents (name TEXT PRIMARY KEY, body TEXT NOT NULL)`, created by
an idempotent `CREATE TABLE IF NOT EXISTS` migration; `put` is an upsert.

### HTTP (src/api.rs)
- `wellknown_aasa` handler: serves the `apple-app-site-association` document
  (shared by the `.well-known` and legacy-root routes).
- `wellknown_assetlinks` handler: serves the `assetlinks.json` document.
  Both: 200 with `content-type: application/json` and the stored body, or 404
  when unset, or 503 on store error.
- `admin_wellknown_get` / `admin_wellknown_put` / `admin_wellknown_delete` on
  `/admin/wellknown/:name`, `admin_guard`-protected. PUT validates name /
  JSON / 64 KiB and stores; GET returns the stored body (or 404); DELETE
  removes it. `:name` is matched against the allowlist
  {`apple-app-site-association`, `assetlinks.json`}; anything else → 404.

### Frontend (web/)
An "App Links" page: two labelled editors (AASA, `assetlinks.json`), each with
load-current / save / clear, inline JSON-validity feedback, and a short note
that these must be served over HTTPS on the redirect domain. Nav entry, i18n
EN + PT-BR, api.ts + queries.ts wiring, a Vitest test for JSON-validity
feedback.

### Docs
`docs/DEEP-LINKING.md` + `docs/DEEP-LINKING.PT_BR.md`: what the files are, how
iOS/Android fetch them, how to produce them, the HTTPS requirement, and an
explicit "device-aware redirect is a follow-up" note. README link; ROADMAP
marks #20 core done + follow-up listed.

## Testing

- Store round-trip (get/put/delete) for LMDB; Postgres round-trip gated by
  `QUARK_TEST_DATABASE_URL`.
- API: PUT then GET the well-known path returns the body with
  `application/json`; unset path returns 404; PUT with non-JSON body → 400;
  PUT with a disallowed name → 404; oversized body → 400; write without admin
  token → 401; the legacy root AASA path serves the same document as the
  `.well-known` one.
- Frontend: JSON-validity feedback on the editor (Vitest).

## Global constraints (inherited)

Code in English; UI i18n EN + PT-BR; docs EN + `PT_BR`; SSRF guard N/A (no
outbound fetch); env `QUARK_ADMIN_TOKEN` behavior unchanged; redirect hot path
pays nothing; no merge to main; avoid-ai-writing on prose; Rust tests with
`-j1`/`CARGO_BUILD_JOBS=1` in this environment.
