# Link password + interstitial — design + plan

**Date:** 2026-07-15
**Branch:** `feat/link-password` (off main; no merge until reviewed)
**Effort:** small-medium. Adds `password_hash` to the link `Record`, an argon2
dependency, an interstitial page + unlock POST on the redirect path, a signed
unlock cookie, rate-limiting, and UI. Bigger than the fallback feature because
it introduces a server-rendered public page and a new crypto dependency.

## Goal

Let an operator protect a short link with a password. A visitor to a protected
link sees a small interstitial page asking for the password; on the correct
password they are redirected (`302`) to the destination and a short-lived signed
cookie lets them skip the form on repeat visits within a window. Unprotected
links (the common case) are completely unaffected and pay **zero** extra cost.

## Scope decisions (author's calls — user was out; documented for review)

- **Opt-in per link**, single password. `Record.password_hash: Option<String>`.
- **Interstitial, not Basic-Auth.** A server-rendered HTML page with a password
  form, POSTing back to the same code. Browser Basic-Auth is uglier, unstyled,
  and cannot carry a "wrong password" message.
- **Plain redirect stays plain.** The password check happens only when
  `password_hash.is_some()`. The hot path for unprotected links reads the field
  and, when `None`, proceeds exactly as today (one `Option::is_none()` check).
- **Not combined with the fallback feature.** An expired link is expired
  regardless of password; expiry is checked first, then password.

## Security decisions (author's calls — documented for review)

- **Hashing: argon2id** via the `argon2` crate (PHC string stored in
  `password_hash`). Correct tool for at-rest password storage: per-hash salt,
  tunable work factor. The verify runs **only on the unlock POST** (rate-limited),
  never on the redirect hot path, so its cost is irrelevant to redirect latency.
  Rejected: HMAC(server_key, password) — no salt/work factor, offline-bruteforceable
  if the store leaks; link passwords are often low-entropy so this matters.
- **Unlock cookie: HMAC-SHA256 signed**, reusing the existing `hmac`/`sha2` deps
  and the server key (`AppState.key` is a `u64`; derive a MAC key from it, or add
  a dedicated signing input — see Task notes). Cookie name `qk_pw_<code>`, value
  `"<expiry_unix>.<base64url(hmac)>"`, `Path=/<code>`, `HttpOnly`, `SameSite=Lax`,
  `Secure` when the request is HTTPS. The MAC covers `code + "." + expiry` so a
  cookie cannot be replayed for another code or past its expiry. TTL default
  **12 hours** (`UNLOCK_TTL_SECS`). Verify with constant-time `Mac::verify_slice`.
- **Rate-limiting:** the unlock POST is rate-limited per client IP via the
  existing `RateLimiter` (`st.ratelimiter.check(ip, now)`), returning `429` when
  over the limit. This throttles password guessing without new infrastructure.
- **No user enumeration / no timing oracle beyond argon2:** a wrong password
  re-renders the form with a generic error; argon2 verify is constant-time.
- **The plaintext password is never stored, never logged, never returned.** The
  API accepts a `password` on create/patch, hashes it, and stores only the hash.
  `LinkRow` exposes `has_password: bool`, never the hash.

## Flow

```
GET /:code
  ├─ resolve + expiry/visit checks (unchanged; expiry wins over password)
  ├─ password_hash == None  → redirect as today (302 / rules / variants)
  └─ password_hash == Some
       ├─ valid unlock cookie present → redirect as today (302)
       └─ else → 200 text/html interstitial (password form)

POST /:code   (form-encoded: password=…)
  ├─ rate-limit (429 if over)
  ├─ resolve; password_hash == None → 404-ish / redirect (treat as normal)
  ├─ argon2 verify(password, hash)
  │    ├─ ok  → 302 to destination + Set-Cookie qk_pw_<code> (signed, 12h)
  │    └─ bad → 200 interstitial again with a generic error (no cookie)
```

The destination resolution after a successful unlock reuses the **exact** existing
logic (app deep-link → rules → variants → url). The interstitial/unlock path only
gates access; it does not change how the destination is picked.

## Non-goals

- Multiple passwords / per-visitor credentials / accounts.
- Password on the API create response or any read path (only `has_password`).
- Remembering unlock across different links (cookie is per-code).
- i18n framework for the public page: the interstitial ships EN + PT-BR strings
  chosen by a simple `Accept-Language` sniff (`pt` → Portuguese, else English).
  This is a public visitor page, not the operator panel; the panel i18n system is
  not involved.

## Tasks

### Task 1 — backend: `Record.password_hash` + argon2 + hash/verify helpers

**Files:** `Cargo.toml` (add `argon2`), `src/store/mod.rs` (Record field),
`src/store/lmdb.rs`, `src/store/postgres.rs` (migration + all read/write sites),
a new `src/password.rs` (hash/verify helpers), `src/lib.rs` (module), tests.

- Add `password_hash: Option<String>` to `Record` with
  `#[serde(default, skip_serializing_if = "Option::is_none")]`.
- Postgres: `ADD COLUMN IF NOT EXISTS password_hash TEXT`; thread through
  `put_link`, `put_alias_and_link`, `get_link`, `list_links`, `search_links`,
  `row_to_link` (all three INSERT column lists **and** placeholder counts — note:
  there are three INSERT statements at different indentation; update all three).
- `src/password.rs`: `hash_password(&str) -> Result<String>` (argon2id, PHC
  string, salt from `getrandom`/`OsRng`) and `verify_password(&str, &str) -> bool`
  (constant-time; false on any parse/verify error). Unit test: hash then verify
  true; wrong password verify false; a known-bad PHC string verifies false without
  panicking.
- Tests: Record round-trips `password_hash`; **regression:** old blob without the
  field deserializes to `None`.

### Task 2 — unlock cookie: sign + verify helper

**Files:** `src/api.rs` (or a small helper section), tests.

- `fn sign_unlock(key, code, expiry) -> String` → base64url(HMAC-SHA256).
- `fn unlock_cookie_valid(headers, key, code, now) -> bool`: parse `qk_pw_<code>`
  from the `Cookie` header, split `expiry.mac`, reject if `expiry <= now`, verify
  the MAC constant-time over `code + "." + expiry`.
- Unit tests: a freshly signed cookie verifies true; a cookie for a different code
  fails; a tampered MAC fails; an expired cookie fails.

### Task 3 — interstitial HTML + redirect gating

**Files:** `src/api.rs` (`redirect` handler + an `interstitial_html` fn), tests.

- In `redirect`, after the expiry/visit checks and after destination resolution
  guards, branch on `rec.password_hash`:
  - `None` → unchanged.
  - `Some` and a valid unlock cookie → unchanged (proceed to 302).
  - `Some` and no valid cookie → return `200 text/html` interstitial with
    `Cache-Control: no-store`.
- `interstitial_html(code, lang, error: bool) -> String`: a minimal self-contained
  styled page (inline CSS, no external assets), form `POST`ing `password` to
  `/{code}`. EN/PT-BR strings by `lang`.
- Tests: protected link, no cookie → `200` + `text/html` + contains the form;
  protected link + valid cookie → `302`; unprotected link → `302` (unchanged).

### Task 4 — unlock POST handler + route

**Files:** `src/api.rs` (new handler + route in `router()`), tests.

- `POST /:code` (form-encoded). Rate-limit via `st.ratelimiter`. Resolve the code;
  if unprotected, treat as a normal visit (redirect). argon2-verify the submitted
  password:
  - ok → `302` to the resolved destination + `Set-Cookie qk_pw_<code>` (signed,
    `UNLOCK_TTL_SECS`, `HttpOnly; SameSite=Lax; Path=/<code>`, `Secure` if HTTPS).
  - bad → `200` interstitial with the error flag set, no cookie.
- Route must not shadow existing `POST /` (create) — the create route is `POST /`
  with no path segment; the unlock is `POST /:code`. Confirm axum precedence keeps
  create working (`POST /` and `POST /:code` are distinct routes).
- Tests (integration, `tests/api_it.rs`): create a protected link (admin);
  `POST /:code` with the right password → `302` + `Set-Cookie`; the returned cookie
  on a follow-up `GET /:code` → `302` (skips the form); wrong password → `200` +
  error, no cookie; over the rate limit → `429`.

### Task 5 — API: accept `password`, expose `has_password`, never leak the hash

**Files:** `src/api.rs` (`CreateReq`, `create_link_core`, `PatchReq`/patch handler,
`LinkRow`), tests.

- `CreateReq` + patch accept an optional `password: Option<String>`. On create,
  a non-empty password is hashed (Task 1) and stored; empty/absent → no password.
  On patch: `null` or empty string clears the password; a non-empty string sets a
  new hash. `create_link_core` gains a `password_hash: Option<String>` parameter
  (the handler hashes before calling core; import passes `None`).
- `LinkRow` gains `has_password: bool` (`= rec.password_hash.is_some()`). The hash
  is never serialized anywhere.
- Tests: create with a password → `GET /admin/links` row has `has_password: true`
  and no hash field; the link then serves the interstitial; patch clears it.

### Task 6 — frontend: password field + protected indicator

**Files:** `web/src/lib/types.ts`, the create/edit dialogs, `web/src/i18n/en.ts`
+ `pt-BR.ts`, `LinkTable.tsx` (indicator), Vitest.

- Types: `Link.has_password?: boolean`; `CreateLinkRequest.password?: string`;
  `PatchLinkRequest.password?: string | null`.
- Dialogs: an optional password input (type=password). Create sends `password`
  when non-empty. Edit: show whether the link is protected; a filled field sets a
  new password, an explicit "remove password" control sends `password: null`.
  (Do not prefill — the hash is not available and must not be.)
- `LinkTable`: a small lock indicator when `has_password`.
- i18n EN + PT-BR. Tests: create sends `password`; edit clears with `null`.

### Task 7 — docs

**Files:** `docs/API.md` + `.PT_BR.md` (create/patch `password`, `has_password`,
the `GET`/`POST /:code` interstitial + `200`/`302`/`429` responses, cookie note),
`docs/ROADMAP` (EN+PT), a short `docs/LINK-PASSWORD.md` (+ PT twin) if the repo
documents features individually.

## Global constraints

- Hot path pays **zero** extra cost for unprotected links (one `Option::is_none`).
- The plaintext password is never stored, logged, or returned; only the argon2
  hash is persisted and only `has_password` is exposed.
- argon2 verify runs only on the unlock POST, never on the redirect hot path.
- Unlock cookie is signed (HMAC-SHA256, server key), per-code, expiring; verified
  constant-time.
- SSRF guard is irrelevant here (no new outbound URL); the destination is the
  link's existing, already-validated URL.
- Code in English; UI i18n EN + PT-BR; the public interstitial ships EN + PT-BR by
  `Accept-Language`; docs EN + PT_BR.
- Non-destructive Postgres migration (`ADD COLUMN IF NOT EXISTS`).
- Old persisted Records without the field must keep working (serde default).
- `-j1` / `CARGO_BUILD_JOBS=1` for Rust builds/tests; kill `quark.exe` before
  building; Postgres tests gated by `QUARK_TEST_DATABASE_URL`.
