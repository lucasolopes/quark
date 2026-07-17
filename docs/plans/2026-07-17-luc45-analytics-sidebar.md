# LUC-45 — Analytics sidebar entry + link selector Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** A dedicated "Analytics" sidebar entry opening a `/analytics` route with a link selector; picking a link renders that link's stats (reusing the existing per-link stats view). The current path (click a link in the table → `/links/:code`) keeps working, rendering the same shared view.

**Architecture:** Extract the body of `LinkStats.tsx` into a shared `StatsView({ code })` component (stats/charts/recent + loading/error states, via `useStats(code)`). Both `/links/:code` and the new `/analytics` render it. The `/analytics` selector is a search `Input` driving `useLinks(term)` (server-side search, graceful client-side fallback on 501), listing matches; picking one sets the selected code in local state → `<StatsView code={selected} />`. Frontend-only, no new backend.

**Tech Stack:** React + TypeScript, react-router-dom, @tanstack/react-query, Vitest. Work under `web/`.

## Global Constraints
- Frontend only; NO backend changes (the `GET /:code/stats` endpoint already exists). `src/codec.rs`/`src/permute.rs` irrelevant (not touched).
- English source; every new user-facing string in BOTH `web/src/i18n/pt-BR.ts` AND `web/src/i18n/en.ts` (`Messages = typeof en`, so `tsc` enforces parity).
- Rendered strings via JSX/`t()` only (React-escaped); no `dangerouslySetInnerHTML`.
- No behavior change to the existing `/links/:code` stats path — it must render the same content it does today (regression test).
- Run the web suite the repo way (inside `web/`: `npx vitest run` — NOT watch), `npx tsc --noEmit`, `npx oxlint --max-warnings 0`. Known pre-existing: one `vite.config.ts` oxlint warning + a flaky `Extensions.test.tsx` under full-suite load — confirm any failure is only those.

## Seams (verified)
- `web/src/routes/LinkStats.tsx`: route component for `/links/:code`; reads `code` from `useParams`, calls `useStats(code)`, renders a back button + heading + `StatsCharts` + `RecentEventsTable` + loading (`StatsSkeleton`)/error states. The body below the back button is what becomes `StatsView`.
- `web/src/components/StatsCharts.tsx`, `web/src/components/RecentEventsTable.tsx`: already-shared presentational pieces `StatsView` composes.
- `web/src/lib/queries.ts`: `useStats(code)` (per-link stats); `useLinks(...)` = `useInfiniteQuery`, `LINKS_PAGE_SIZE`, supports a server-side search term (backend may 501 → fall back to client-side filter of loaded pages).
- `web/src/app/Shell.tsx`: `navGroups` (~line 32); the group holding Pixels (`navGroupData`) is where the Analytics item goes; each item is `{ to, label, icon }` (lucide icon).
- `web/src/app/router.tsx`: route registration (add `/analytics`).
- `web/src/i18n/{en,pt-BR}.ts`: `shell.nav*` keys + a `stats.*` group.
- Only `input.tsx` exists under `components/ui/` — no combobox/select/popover primitive; build the selector from `Input` + a results list.

## File Structure
- Create `web/src/components/StatsView.tsx` (the shared stats body).
- Modify `web/src/routes/LinkStats.tsx` (render `StatsView`).
- Create `web/src/routes/Analytics.tsx` (selector + StatsView).
- Modify `web/src/app/Shell.tsx` (nav item), `web/src/app/router.tsx` (route), `web/src/i18n/en.ts` + `pt-BR.ts` (keys).
- Tests: `web/src/components/StatsView.test.tsx` (or fold into existing LinkStats test), `web/src/routes/Analytics.test.tsx`.

---

### Task 1: Extract `StatsView({ code })` shared component

**Files:** Create `web/src/components/StatsView.tsx`; Modify `web/src/routes/LinkStats.tsx`; Test `web/src/routes/LinkStats.test.tsx` (or a new `StatsView.test.tsx`).

**Produces:** `export function StatsView({ code }: { code: string })` — renders the per-link stats body (heading/subtitle, loading `StatsSkeleton`, error card, and on success the `StatsCharts` + `RecentEventsTable` + summary cards), driven by `useStats(code)`. It renders NO back button and NO route-level chrome (those stay in the route wrappers).

**Steps:**
- [ ] Read `LinkStats.tsx` fully. Identify the body below the back-button header (the `useStats` query + all its render branches).
- [ ] Write/adjust a failing test: rendering `<StatsView code="abc" />` with a mocked `useStats` (or mocked fetch) shows the stats content (e.g. a known metric/heading); rendering while pending shows the skeleton. (If an existing `LinkStats.test.tsx` covers this, adapt it to assert the same content still renders through `StatsView`.)
- [ ] Create `StatsView.tsx` with the extracted body. Keep the `stats.*` i18n keys it already uses. The heading/subtitle (`stats.heading`/`stats.subtitle`) move into `StatsView` (both routes want them); the back button stays in `LinkStats`.
- [ ] Refactor `LinkStats.tsx` to render the back button + `<StatsView code={code} />` — no duplicated stats logic.
- [ ] Run `npx vitest run` (the stats/links tests) + `npx tsc --noEmit`; confirm `/links/:code` still renders stats (regression). Commit `refactor(web): extract StatsView({code}) shared stats body from LinkStats`.

---

### Task 2: `/analytics` route + sidebar entry + link selector

**Files:** Create `web/src/routes/Analytics.tsx`; Modify `web/src/app/Shell.tsx`, `web/src/app/router.tsx`, `web/src/i18n/en.ts`, `web/src/i18n/pt-BR.ts`; Test `web/src/routes/Analytics.test.tsx`.

**Interfaces:** Consumes `StatsView({ code })` from Task 1.

**Steps:**
- [ ] Add i18n keys to BOTH locales under a new `analytics` group: `analytics.heading`, `analytics.searchPlaceholder` (e.g. "Search a link…"), `analytics.empty` ("Select a link to see its analytics"), `analytics.noResults` ("No links found"); and `shell.navAnalytics` ("Analytics"). Match existing key style.
- [ ] Add the nav item to `Shell.tsx`'s Analytics/Data group: `{ to: "/analytics", label: t("shell.navAnalytics"), icon: <a lucide chart icon, e.g. BarChart3> }`. Register `/analytics` → `Analytics` in `router.tsx` (inside the authed `RequireAuth`/Shell layout, like the other routes).
- [ ] Write failing `Analytics.test.tsx`: (a) with no selection, the empty state shows and no StatsView content; (b) typing in the search Input queries links (mock `useLinks`/fetch) and lists matches; (c) clicking a result renders `StatsView` for that code (mock `useStats`); (d) picking a different result swaps the stats without a full reload (the selected code state changes). 
- [ ] Implement `Analytics.tsx`: a heading, a search `Input` (local `term` state) feeding `useLinks(term)`; render the flattened link pages as a selectable list (code + destination url, click sets `selected`); `useLinks` 501/search-unsupported → filter the loaded pages client-side by code/url substring (graceful). Below: if `selected` → `<StatsView code={selected} />`, else the empty state. Keep it keyboard-accessible (buttons for results, `aria-label`s).
- [ ] Run `npx vitest run` (full or at least Analytics + Shell + LinkStats) + `tsc` + `oxlint`; confirm the sidebar shows Analytics and the flow works. Commit `feat(web): /analytics route with link selector reusing StatsView (LUC-45)`.

## Verification (whole-plan)
- `StatsView` renders the per-link stats; `/links/:code` unchanged (regression); `/analytics` lists links via search, picks one, renders its stats, swaps without reload; empty/no-results states; i18n parity (tsc); a11y (button/aria on results). Web suite green (modulo the known Extensions flake); tsc + oxlint clean. Then a whole-branch review before merge.
