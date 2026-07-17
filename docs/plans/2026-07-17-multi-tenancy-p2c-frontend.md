# Multi-tenancy P2c-frontend (invites UI) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Complete team invites end to end in the panel: a Members page (Owner/Admin create/list/revoke invites, copy the accept link) and a public `/invite/:token` accept page. Consumes the merged P2c-backend (`/admin/invites` create/list/revoke + `/admin/invites/:token/accept`). Decision-independent; OSS shows neither.

**Architecture:** Extend `lib/types.ts`/`lib/api.ts`/`lib/queries.ts` with invite types + methods + hooks. Add a `Members` route under the `Shell`-wrapped tree, shown in the sidebar only in cloud when the current membership role is Owner/Admin. Add an `AcceptInvite` route at `/invite/:token` OUTSIDE the `RequireAuth` tree (an invitee has no workspace yet and must not be trapped in `Onboarding`); it runs its own auth check and redirects unauthenticated users to `/login`. Mirrors the P2b-frontend patterns (workspace onboarding/switcher).

**Tech Stack:** React 19, TypeScript, Vite, TanStack Query, react-router-dom, Tailwind + `components/ui/*`, i18n (en + pt-BR), Vitest + Testing Library.

## Global Constraints

- Code/identifiers English; user-facing copy via i18n (en + pt-BR, matching key shapes); prose follows avoid-ai-writing (no em dashes, no AI-isms).
- **Cloud-only surfaces:** the Members nav item + page only render in cloud (`me.memberships !== undefined`) AND when the current tenant's role is `Owner`/`Admin`. In OSS nothing invite-related shows. A test asserts OSS/non-admin hides it.
- No backend changes (P2c-backend is merged: `POST /admin/invites {email, role}` → `{id, token, email, role, expires}`; `GET /admin/invites` → `[{id, email, role, expires, created}]`; `DELETE /admin/invites/:id`; `POST /admin/invites/:token/accept` → 200 `{tenant_id, role}` / 400 / 401 / 403 / 404 / 409). The create response returns `token` (no url) — the frontend builds the accept link as `${window.location.origin}/invite/${token}`.
- Tests colocated `*.test.tsx`, `withProviders` from `@/test-utils`, fetch spy `vi.spyOn(globalThis,"fetch")` — same as `Login.test.tsx`/`RequireAuth.test.tsx`.
- Verification gate per task (from `web/`): `npm run typecheck` + `npm run lint` (oxlint `--max-warnings 0`, ignore only the pre-existing `vite.config.ts` warning) + `npm run test`.

## Frontend seams (from the P2c map + P2b-frontend)

- `web/src/app/router.tsx` — `/login` is the only route OUTSIDE the `RequireAuth`+`Shell` tree. Add `/invite/:token` as a second outside route.
- `web/src/app/Shell.tsx` — `navGroups` (sidebar) + the `WorkspaceSwitcher` in the header (P2b). Add a Members nav item.
- `web/src/app/RequireAuth.tsx` + `web/src/app/WorkspaceGate.tsx` + `web/src/routes/Onboarding.tsx` — the "no workspace yet" precedent; the accept page must NOT be nested under this.
- `web/src/lib/{types.ts,api.ts,queries.ts}` — `MeResponse.memberships[].role` + `current_tenant` already exist (P2b). `useMe()`, `useSwitchWorkspace()` exist.
- `web/src/routes/Login.tsx` — pattern for a full-screen card + `oidcLoginUrl()`; the accept page redirects unauthenticated users here.

## File Structure

- Modify `web/src/lib/types.ts`, `web/src/lib/api.ts`, `web/src/lib/queries.ts`.
- Create `web/src/routes/Members.tsx` (+ test), `web/src/components/InviteDialog.tsx` (+ test) OR fold the dialog into Members.
- Create `web/src/routes/AcceptInvite.tsx` (+ test).
- Modify `web/src/app/router.tsx`, `web/src/app/Shell.tsx`.
- Modify `web/src/i18n/en.ts`, `web/src/i18n/pt-BR.ts`.

---

### Task 1: types + API + hooks + i18n

**Files:** `lib/types.ts`, `lib/api.ts`, `lib/queries.ts`, `i18n/en.ts`, `i18n/pt-BR.ts`; test append `lib/api.test.ts`.

**Interfaces produced:**
- `interface InviteView { id: number; email: string; role: string; expires: number; created: number }`
- `interface CreateInviteResponse { id: number; token: string; email: string; role: string; expires: number }`
- `api.listInvites(): Promise<InviteView[]>` (`GET /admin/invites`); `api.createInvite(email: string, role: string): Promise<CreateInviteResponse>` (`POST /admin/invites`); `api.revokeInvite(id: number): Promise<void>` (`DELETE /admin/invites/:id`); `api.acceptInvite(token: string): Promise<{tenant_id: number; role: string}>` (`POST /admin/invites/:token/accept`). All state-changing calls send the `x-quark-csrf` header + `credentials:include` via the existing `req` helper.
- `useInvites()` (query `["invites"]`), `useCreateInvite()` (invalidates `["invites"]`), `useRevokeInvite()` (invalidates `["invites"]`), `useAcceptInvite()` (on success invalidates ALL queries incl. `["me"]` — the user just joined a tenant).
- i18n `invites.*` (title, empty, inviteButton, emailLabel, roleLabel, roleAdmin/Member/Viewer, create, copyLink, linkCopied, revoke, pending, expires, errors: slugTaken n/a; `accept.*` (title, description, acceptButton, accepting, success, errorExpired, errorEmailMismatch, errorAlreadyMember, errorGeneric, signInFirst)) + a `shell.navMembers` key. Both en and pt-BR, identical shape.

**Steps:**
- [ ] Add the types to `types.ts`.
- [ ] Add the four methods to `api.ts` (follow the `createWorkspace`/`switchWorkspace` idiom: `jsonOrThrow(await req(...))` for JSON returns; `req` + `if (!res.ok) throw new ApiError(...)` for void). `acceptInvite` posts to `/admin/invites/${encodeURIComponent(token)}/accept`.
- [ ] Add the hooks to `queries.ts` (mirror `useCreateWorkspace`: `useAcceptInvite` calls `client.invalidateQueries()` with no key on success).
- [ ] Add i18n keys to en.ts + pt-BR.ts.
- [ ] Write `lib/api.test.ts` cases: `createInvite` posts `{email, role}` and returns token; `acceptInvite` posts to the token path; `acceptInvite` throws `ApiError(409)` on already-member.
- [ ] `npm run typecheck && npm run lint && npm run test` green.
- [ ] Commit `feat(web): invite types + API client + query hooks + i18n`.

---

### Task 2: Members page (list + create dialog + revoke) in the Shell

**Files:** Create `web/src/routes/Members.tsx` (+ `Members.test.tsx`); Modify `web/src/app/router.tsx` (add `members` child route), `web/src/app/Shell.tsx` (nav item).

**Interfaces consumed:** `useInvites`/`useCreateInvite`/`useRevokeInvite` (T1), `useMe` (role gating), `Button`/`Dialog`/`Input`/`Table`/dropdown from `components/ui`, `useT`.

**Steps:**
- [ ] Write `Members.test.tsx` (failing): renders the pending invites list from a mocked `GET /admin/invites`; clicking "invite", filling email + selecting role, submitting posts to `/admin/invites` and shows the copyable accept link (`${window.location.origin}/invite/<token>`); revoke calls DELETE; the page shows an admin-only empty/forbidden state if `me` role is Viewer/Member (or the nav item is hidden — assert whichever you implement).
- [ ] Run, confirm fail.
- [ ] Implement `Members.tsx`: a table of pending invites (email, role, expires, created); an "Invite" button opening a `Dialog` with an email `Input` + a role select (Admin/Member/Viewer — NOT Owner); on submit call `useCreateInvite`; on success render the accept link `${window.location.origin}/invite/${resp.token}` with a copy button (reuse the copy idiom from `LinkTable`); a revoke action per row (`useRevokeInvite`). Handle 429 (`common.rateLimited`) + generic errors. Role helper maps the role string to the i18n label.
- [ ] Wire the route: add `{ path: "members", element: <Members /> }` under the authed tree in `router.tsx`.
- [ ] Wire the nav: in `Shell.tsx`, add a Members item to the appropriate `navGroups` group, rendered ONLY when cloud (`me.memberships !== undefined`) AND the current tenant role ∈ {Owner, Admin} (derive from `me.memberships.find(m => m.tenant_id === me.current_tenant)?.role`). Use an appropriate lucide icon (e.g. `Users`).
- [ ] Run tests green; typecheck/lint/test.
- [ ] Commit `feat(web): Members page — create/list/revoke invites (Owner/Admin, cloud-only)`.

---

### Task 3: AcceptInvite route (public, outside RequireAuth)

**Files:** Create `web/src/routes/AcceptInvite.tsx` (+ `AcceptInvite.test.tsx`); Modify `web/src/app/router.tsx`.

**Interfaces consumed:** `useMe` (auth check), `useAcceptInvite` (T1), `oidcLoginUrl`, `Card`/`Button` from `components/ui`, `useNavigate`/`useParams`.

**Steps:**
- [ ] Write `AcceptInvite.test.tsx` (failing): unauthenticated (`me.authenticated=false`) → renders a "sign in to accept" state with a link/button to login (does not auto-accept); authenticated → shows the accept card and an "accept" button; clicking accept posts to `/admin/invites/:token/accept` and on 200 navigates into the app (`/links`); a 403 shows the email-mismatch message; 409 shows already-member; 404/410 shows expired/invalid.
- [ ] Run, confirm fail.
- [ ] Implement `AcceptInvite.tsx`: read `:token` via `useParams`; `const me = useMe()`. While loading → spinner. If `!me.data?.authenticated` → a full-screen card explaining they must sign in, with a button to `oidcLoginUrl()` (or navigate `/login`) — do NOT auto-accept (the user must be the invited identity). If authenticated → a card "You've been invited to a workspace" + an Accept button calling `useAcceptInvite(token)`; on success invalidate all queries (hook does it) + `navigate("/links", { replace: true })`; map errors: 403 → email-mismatch copy, 409 → already-member (offer a link to the app), 404 → invalid/expired, 429 → rate-limited. No workspace-gate interference (this route is outside `RequireAuth`).
- [ ] Wire the route OUTSIDE the authed tree in `router.tsx`: `{ path: "/invite/:token", element: <AcceptInvite /> }` as a sibling of `/login` (NOT under the `RequireAuth`/`Shell` element).
- [ ] Run tests green; typecheck/lint/test.
- [ ] Commit `feat(web): /invite/:token accept page (public route, own auth check)`.

---

## Self-Review

- Spec coverage: types/API/hooks/i18n (T1); Members create/list/revoke + nav gating (T2); public accept page outside RequireAuth (T3).
- Placeholder scan: the accept-link base is `window.location.origin` (panel origin) — concrete, no TBD. Role labels via i18n.
- Type consistency: `InviteView`/`CreateInviteResponse` + the four api methods + hook names are used identically across T1-T3. `me.memberships`/`current_tenant` (from P2b) drive the nav gating.
- OSS/non-admin: Members nav hidden unless cloud + Owner/Admin (T2); accept route works regardless but only grants via the backend's checks.

## Verification (whole-plan)

- `npm run build` + `npm run test` + `npm run lint` green from `web/`.
- Manual/controller: cloud Owner invites → copies link → (as the invited identity) opens `/invite/:token` → accepts → lands in the workspace; a non-invited email → 403 message; OSS shows no Members nav.
