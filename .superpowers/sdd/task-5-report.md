# LUC-57 Task 5 report: admin UI for SSO email domains

Commit: `b4092f8` on `feat/sso-email-discovery` (frontend-only, no backend changes).

## What changed

- `web/src/lib/types.ts`: `DomainStatus` (`"pending" | "verified"`) and `SsoDomainView` (`id, domain, status, created, verified_at, txt_name, txt_value`), matching `SsoDomainView`/`sso_domain_view` in `src/api.rs`.
- `web/src/lib/api.ts`: `listSsoDomains()`, `createSsoDomain(domain)`, `verifySsoDomain(id)`, `deleteSsoDomain(id)` next to the existing CRUD-style methods. Also `oidcConfigured(): Promise<boolean>` — a plain (non-throwing) fetch to `GET /admin/oidc-config`, mapping 200→true / 404→false, mirroring the existing `sheetsStatus` pattern (a workspace with no OIDC provider yet is a normal `false`, not a 401 to bounce the panel on).
- `web/src/lib/queries.ts`: `useSsoDomains` (list query), `useCreateSsoDomain`, `useVerifySsoDomain`, `useDeleteSsoDomain` (mutations invalidating the list), and `useOidcConfigured` (the gating query, `retry: false`).
- `web/src/routes/SsoDomains.tsx` (new): the admin screen.
  - Gate: renders `null` when `me().memberships === undefined` (OSS — mirrors `RequireAuth`/`Shell`'s existing cloud-detection). When cloud, calls `GET /admin/oidc-config` via `useOidcConfigured`; while pending shows a skeleton, when `false` shows a short "set up an SSO provider first" message with no domain list, when `true` renders the full panel. This satisfies "cloud + SSO-configured" using the real backend signal (`GET /admin/oidc-config`, already shipped in Task 2/P2d, just not previously wired to the frontend) rather than approximating it — there was no existing P2d admin UI in `web/src/` to mirror, so I used the endpoint itself as the more precise precedent per the brief's fallback guidance ("mirror whatever the OIDC-config admin UI does, OR gate on cloud and let calls 409 gracefully").
  - Panel: list of domains as cards (domain, status badge, created date, Verify button for pending rows, Remove button always); a pending domain's card also shows its `_quark-sso.<domain>` TXT name/value inline (plain JSX text, no `dangerouslySetInnerHTML`); an "Add domain" dialog (mirrors `Members`'/`Webhooks`' create-dialog pattern) with client-side domain-shape validation and 409/400/429 error mapping; a remove confirmation `AlertDialog` (mirrors `Members`' revoke flow).
- `web/src/app/router.tsx`: route `sso-domains` → `<SsoDomains />`, alongside the existing `members` route.
- `web/src/app/Shell.tsx`: nav item "SSO domains" (`ShieldCheck` icon) in the Dev group, gated the same way as the existing Members item (`canManageSsoDomains = canManageMembers`, i.e. cloud + Owner/Admin) — the SSO-domains screen's own internal gate (above) is what enforces "SSO configured".
- i18n: `shell.navSsoDomains` + a new `ssoDomains` namespace (title, subtitle, not-configured message, empty state, add form, column headers, status labels, TXT-record instructions/labels, verify/remove actions and their toasts, error mappings for 400/409/429) added to both `web/src/i18n/en.ts` and `web/src/i18n/pt-BR.ts`.
- Tests:
  - `web/src/lib/api.test.ts` (+8 tests): `oidcConfigured` true (200) / false (404); `listSsoDomains` GETs `/admin/sso-domains`; `createSsoDomain` POSTs `{domain}` to `/admin/sso-domains`; `verifySsoDomain` POSTs to `/admin/sso-domains/:id/verify`; `deleteSsoDomain` DELETEs `/admin/sso-domains/:id` + throws `ApiError` on a non-ok response.
  - `web/src/routes/SsoDomains.test.tsx` (new, 7 tests): OSS (no `memberships`) renders nothing; cloud without an SSO provider configured shows the not-configured message and no domain list; cloud+configured lists a pending and a verified domain, with the TXT record shown only for the pending one; empty state; clicking Verify calls the verify endpoint and the list refetches; the add form posts the trimmed domain; Remove asks for confirmation then DELETEs.

## Test command + output

```
cd web
npx vitest run
```

Full-suite result: **2 failed | 190 passed (192)**, both failures in `src/routes/Extensions.test.tsx` ("create flow calls the API" / a setup-Zapier click assertion) — confirmed pre-existing and unrelated to this change:
- A first full run (accidentally concurrent with a separate `tsc --noEmit` invocation) also transiently failed `Webhooks.test.tsx` on a timeout; re-ran `Webhooks.test.tsx` + `Extensions.test.tsx` together and `Webhooks.test.tsx` alone — it passed cleanly every time in isolation, confirming that failure was resource contention from running two heavy processes in parallel, not a real regression.
- Re-ran the full suite alone (no other process running): consistently 190/192, the same 2 `Extensions.test.tsx` failures, matching the documented pre-existing flake.
- `SsoDomains.test.tsx` and `api.test.ts` on their own: 7/7 and 29/29 green, including a final standalone run after the last Shell.tsx cleanup.

```
npx tsc --noEmit
```
No output (clean).

```
npx oxlint
```
One pre-existing warning, unrelated to this change (`vite.config.ts:1:1: triple-slash-reference`). My test file's own transient warning (`method` declared but unused in an overridden mock) was fixed before the final run — final `oxlint` run shows only the pre-existing warning.

## Design decision worth flagging

The brief's "gate to cloud + SSO-configured" left the exact signal open (no P2d admin UI existed to mirror in `web/src/`). I found the backend already exposes `GET /admin/oidc-config` (404 when unset, 200 otherwise, cloud-only, `Scope::Full`) — unused by any existing frontend code — and used it directly as the gating signal instead of approximating with cloud-only + graceful 409 handling. This is a small, self-contained addition (`api.oidcConfigured()` + `useOidcConfigured()`), reuses an existing endpoint rather than inventing a signal, and gives an exact answer instead of a heuristic. Flagging in case the reviewer would rather see the simpler cloud-only + 409-toast approach the brief also explicitly permitted.

## Concerns

- No existing P3 custom-domains admin UI was found in `web/src/` (confirmed by grep for `domains`/`Domains` across `web/src`) — I mirrored `Members.tsx`/`Webhooks.tsx` instead, per the brief's fallback instruction.
- The nav item's role gate (Owner/Admin only) reuses `Members`' `MEMBERS_MANAGER_ROLES` set. I confirmed `admin_guard` in `src/api.rs` checks scope, not role, directly for `Scope::Full`; I did not trace how OIDC-session roles map to scopes for every possible role. This mirrors the exact same (pre-existing) assumption `Shell.tsx` already makes for the Members nav item, so it is not a new risk introduced by this task — worth a second look together with Members' gating, not in isolation.
