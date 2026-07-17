# P2d-frontend (org-aware login + model-B invite) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Wire the frontend so the P2e model-B flow works end to end: accepting an invite that returns `login_required` sends the browser to the tenant's SSO, and the Login screen offers a per-tenant "sign in to `<org>`" entry when `?org=` is present. Everything else (discovery, onboarding, workspace switcher) already exists.

**Architecture:** Two small, independent frontend changes. Task 1 makes `acceptInvite` surface its response body and redirects the browser to the API's `/admin/login?org=<slug>` when the accept returns model-B `login_required`. Task 2 makes `Login.tsx` read `?org=` from its own query string and, when present, drive the OIDC button at that tenant. No backend changes.

**Tech Stack:** React + TypeScript, react-router-dom, @tanstack/react-query, Vitest. All work under `web/`.

## Global Constraints
- Code + copy in English (source); user-facing strings go through i18n with both `pt-BR` and `en` keys. Follow avoid-ai-writing for any prose.
- Cloud-aware, NO backend changes. Model-A (no `status` in accept response) and OSS behavior must be byte-for-byte unchanged — regression tests prove it.
- `?org=` from the query is an untrusted UX hint only; the frontend just forwards it. The backend already validates the slug and returns a generic 404 for unknown/no-config tenants (anti-enumeration) — the frontend must not branch on slug existence or leak it.
- The invite redirect and the org login button must use a full-page navigation to the API origin (`window.location.assign`), not `fetch` — a browser redirect avoids CORS (panel and API are separate origins).
- i18n interpolation is `t("key", { org: slug })` with `{org}` placeholders (via `interpolate` in `web/src/i18n/shared.ts`). Add keys to BOTH `web/src/i18n/pt-BR.ts` and `web/src/i18n/en.ts`.
- Run the web suite with the repo's usual command (`npm test` / `vitest run` inside `web/`); keep `Login.test.tsx` and `AcceptInvite.test.tsx` green.

## Seams (verified)
- `web/src/lib/api.ts`: `oidcLoginUrl()` → `` `${BASE}/admin/login` ``; `BASE` is the API base. `acceptInvite` currently resolves to `void` (discards the 200 body). `me()`, `createWorkspace`, `switchWorkspace` unchanged.
- `web/src/lib/queries.ts`: `useAcceptInvite` wraps `api.acceptInvite`.
- `web/src/routes/AcceptInvite.tsx`: `handleAccept` → `acceptInvite.mutate(token, { onSuccess: () => navigate("/links") })`. Error mapping (403/409/404/410/429) stays.
- `web/src/routes/Login.tsx`: token field + OIDC button (`oidcLoginUrl()`, no org) shown when `me().oidc_enabled`. Uses `useNavigate`; add `useSearchParams`.
- `web/src/i18n/{pt-BR,en}.ts`: `login.*` and `accept.*` key groups already exist.
- Backend contract (do not change): accept model B → `200 {"status":"login_required","login_url":"/admin/login?org=<slug>"}`; accept model A → 200 with the existing body (no `status`). `/admin/login?org=<slug>` is the per-tenant SSO redirect endpoint.

## File Structure
- Modify `web/src/lib/api.ts` (oidcLoginUrl signature + acceptInvite return type).
- Modify `web/src/routes/AcceptInvite.tsx` (handle `login_required`).
- Modify `web/src/routes/Login.tsx` (read `?org=`, org-aware OIDC button).
- Modify `web/src/i18n/pt-BR.ts` + `web/src/i18n/en.ts` (new keys).
- Tests: `web/src/lib/api.test.ts`, `web/src/routes/AcceptInvite.test.tsx`, `web/src/routes/Login.test.tsx`.

---

### Task 1: `acceptInvite` surfaces its body + AcceptInvite redirects to tenant SSO on `login_required`

**Files:** Modify `web/src/lib/api.ts`, `web/src/routes/AcceptInvite.tsx`; Test `web/src/lib/api.test.ts`, `web/src/routes/AcceptInvite.test.tsx`.

**Interfaces:**
- Produces: `oidcLoginUrl(org?: string): string` — `${BASE}/admin/login` with no arg; `${BASE}/admin/login?org=${encodeURIComponent(org)}` with an org. (Task 2 consumes this.)
- Produces: `api.acceptInvite(token)` now resolves to `{ status?: string; login_url?: string }` (the parsed 200 body) instead of `void`. Model-A responses have no `status`.

**Steps:**
- [ ] Read `oidcLoginUrl`, `acceptInvite`, and `req`/`BASE` in `web/src/lib/api.ts`, and the `acceptInvite` test in `api.test.ts`, to match the existing style.
- [ ] Write failing tests in `api.test.ts`: `oidcLoginUrl("acme")` contains `/admin/login?org=acme`; `oidcLoginUrl()` ends with `/admin/login` and has no `?org`; `acceptInvite` returns the parsed body (assert it returns `{status:"login_required",login_url:"/admin/login?org=acme"}` when the mocked fetch responds with that JSON, and returns the model-A body with no `status` otherwise). Run, confirm fail.
- [ ] Implement: change `oidcLoginUrl` to accept an optional `org` and append `?org=${encodeURIComponent(org)}` when truthy. Change `acceptInvite` to `return jsonOrThrow<{ status?: string; login_url?: string }>(await req(...))` (model A's body deserializes fine — extra fields ignored, `status` absent). Keep the method/path/body identical.
- [ ] Run `api.test.ts`; confirm pass.
- [ ] Write failing tests in `AcceptInvite.test.tsx`: (a) when `acceptInvite` resolves `{status:"login_required",login_url:"/admin/login?org=acme"}`, the component calls `window.location.assign` with a URL ending in `/admin/login?org=acme` (spy/stub `window.location.assign`) and does NOT navigate to `/links`; (b) when it resolves a model-A body (no `status`), it navigates to `/links` (regression); (c) the 403/409/404/410 error copy is unchanged. Run, confirm fail.
- [ ] Implement in `AcceptInvite.tsx`: in the `acceptInvite.mutate` `onSuccess(data)`, if `data?.status === "login_required" && data.login_url`, compute the absolute URL by prefixing the API base (reuse the same base `oidcLoginUrl`/`req` use — export a small helper or `oidcLoginUrl`-style constant from `api.ts` if `BASE` isn't already importable; do NOT hardcode) and call `window.location.assign(absoluteUrl)`; else `navigate("/links", { replace: true })`. Leave error handling untouched.
- [ ] Run `AcceptInvite.test.tsx`; confirm pass.
- [ ] Run the full web suite; commit `feat(web): accept-invite follows model-B login_required to the tenant SSO`.

---

### Task 2: `Login.tsx` reads `?org=` and offers a per-tenant sign-in

**Files:** Modify `web/src/routes/Login.tsx`, `web/src/i18n/pt-BR.ts`, `web/src/i18n/en.ts`; Test `web/src/routes/Login.test.tsx`.

**Interfaces:**
- Consumes: `oidcLoginUrl(org?)` from Task 1.

**Steps:**
- [ ] Read `Login.tsx` and the `login.*` key group in `pt-BR.ts`/`en.ts`, plus one existing interpolated key (e.g. `linkTable.deleteTitle`) to copy the `{org}` interpolation style.
- [ ] Add i18n keys to BOTH `pt-BR.ts` and `en.ts` under `login`: `orgButton` (pt: `"Entrar em {org}"`, en: `"Sign in to {org}"`) and `orgHeader` (pt: `"Organização: {org}"`, en: `"Organization: {org}"`).
- [ ] Write failing tests in `Login.test.tsx`: rendering at `/login?org=acme` (wrap in a router with the search param) shows the org header/button copy containing "acme", and clicking the provider button sets `window.location.href` to a URL containing `/admin/login?org=acme` (stub `window.location`); rendering at `/login` with no `org` shows the shared button and the click hits `oidcLoginUrl()` with no `?org` (regression); the token field is present in both. Run, confirm fail.
- [ ] Implement in `Login.tsx`: `const [params] = useSearchParams(); const org = params.get("org")?.trim() || "";`. When `org` is non-empty and `oidcEnabled`: render `t("login.orgHeader", { org })` and a button labeled `t("login.orgButton", { org })` whose click does `window.location.href = oidcLoginUrl(org)`. When `org` is empty: the existing shared button (`oidcLoginUrl()`), unchanged. The token form stays in both branches.
- [ ] Run `Login.test.tsx`; confirm pass.
- [ ] Run the full web suite; commit `feat(web): org-aware login — ?org= drives per-tenant SSO sign-in`.

## Verification (whole-plan)
- `oidcLoginUrl` builds `/admin/login` (shared) and `/admin/login?org=<slug>` (per-tenant) correctly.
- Model-B invite accept redirects to the tenant SSO; model-A accept still lands in `/links` (regression); OSS unaffected.
- `Login.tsx` with `?org=` offers per-tenant sign-in; without it, the shared login is unchanged.
- Full `web/` Vitest suite green; no regression in `Login.test.tsx`/`AcceptInvite.test.tsx`. Then a whole-branch review before merge. Host-derived org + tenant-host-served panel remain a separate future brick.
