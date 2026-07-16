# P2b-backend Task 5 report

## Status: DONE

Commit: `3b55e7428cb305771470e730a59db22356392fae` on `feat/multi-tenant-p2b`
(parent `0a2ae67`).

## What changed

`admin_guard`'s OIDC-session branch in `src/api.rs` (the `Ok(Some(session))`
arm). The covering-check now sources scopes by deployment mode:

- **Cloud (`st.multi_tenant == true`)**: `get_membership(session.user_id,
  session.tenant_id)` → `Ok(Some(m))` uses `role_scopes(m.role)`; `Ok(None)`
  (no membership in the current tenant) sets `saw_insufficient` and yields no
  scopes → 403; `Err(_)` sets `saw_store_error` → 503-if-nothing-else-covers.
- **OSS (`multi_tenant == false`)**: unchanged — `session.scopes.clone()` (the
  group→scope map), byte-for-byte the old behavior.

The success return still carries `Principal { tenant: session.tenant_id,
user_id: Some(session.user_id), scopes: effective_scopes }`. The
`saw_insufficient / saw_rate_limited / saw_store_error / not_found_status` tail
and the "try every credential, any covering one wins" ordering are untouched;
only the scope SOURCE inside the OIDC-session covering-check moved. Env-admin-token
and API-token branches are byte-for-byte identical.

## Tests

Full run green: **228 lib + 89 api_it + 4 tenant_enforcement** (the tenant
suite run against real Postgres, superuser). `cargo fmt --check` clean. The
existing OSS status-contract unit test `admin_guard_resolves_principal_per_credential`
stays green (OSS path unchanged).

New gated test `admin_guard_role_scopes_in_cloud` in `tests/tenant_enforcement.rs`
drives the real HTTP router (cloud AppState: `multi_tenant=true`,
`oidc_configured=true`, no env token) over a Postgres store:
- Viewer membership + session (whose stored `session.scopes` is deliberately
  `Full`): `GET /admin/links` (LinksRead) → 200; `POST /` create (LinksWrite) →
  403. Proves stored session scopes are ignored in cloud.
- Session whose user has NO membership in `session.tenant_id`: `GET /admin/links`
  → 403 (never authorizes).
- Early-returns when `QUARK_TEST_DATABASE_URL` is unset (controller runs the
  gated arm).

TDD verified against live Postgres: with `src/api.rs` stashed the test FAILS
(Viewer write returns 200; orphan authorizes), and PASSES with the fix. NO
CONCURRENTLY used; codec/permute untouched; `/admin/me` (Task 6) not touched.

## Concerns

- `admin_guard` is private, so the gated test exercises it end-to-end through
  the public `router()` HTTP surface (session cookie → `GET /admin/links` /
  `POST /`) rather than calling it directly. Stronger (real wiring) but coupled
  to those two routes' scope requirements (LinksRead / LinksWrite via
  `require_admin_for_create`).
- Test note: `Session.expires` must stay within i64 (the BIGINT column);
  `u64::MAX` wraps to -1 and reads back as expired. The helper uses
  `4_000_000_000`.

## Follow-up: Minor status-contract divergence fixed (2026-07-16)

**Finding:** the post-covering-check flag-set in the OIDC-session branch had
drifted to `if !effective_scopes.is_empty() { saw_insufficient = true; }`
(`src/api.rs` ~line 1425). An OSS session with EMPTY `session.scopes` left the
flag unset, falling through to `not_found_status` (401) instead of the
original 403. Masked today because the OIDC callback rejects empty-scope
logins, but it coupled the guard's own status contract to an invariant living
in another function — a latent 403→401 divergence.

**Fix:** restored the unconditional form — `saw_insufficient = true;`
unconditionally after the covering check fails, matching the original
byte-for-byte behavior. Verified correct for all four paths:
- OSS covering: unaffected (early return before this line).
- OSS empty-scope: now 403 again (was silently 401).
- Cloud no-membership (`Ok(None)`): already set `saw_insufficient=true`;
  setting it again is a no-op.
- Cloud store-error (`Err(_)`): sets `saw_store_error=true`; the tail checks
  `saw_store_error` before `saw_insufficient`, so 503 still wins.

Added unit test `admin_guard_oss_empty_scope_session_is_forbidden_not_unauthorized`
in `src/api.rs` `#[cfg(test)]`: seeds an OSS session (`multi_tenant=false`,
`oidc_configured=true`) with `scopes: Vec::new()` and asserts `admin_guard`
returns `Err(StatusCode::FORBIDDEN)`, locking the invariant directly at the
unit level (no gated harness needed).

Verification: `cargo fmt --check` clean; full non-gated suite green (229
passed, up from 228 — the new test — 0 failed); `admin_guard_resolves_principal_per_credential`
and `admin_guard_role_scopes_in_cloud` (tenant_enforcement, Postgres-gated)
both still green. No other lines in the branches or the tail touched;
codec/permute untouched.

Status: DONE.
