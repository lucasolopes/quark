# UTM builder + templates — design (roadmap #8)

**Date:** 2026-07-14 · **Branch:** feat/utm-builder (off main; no merge) · **Effort:** low · **Frontend only** (no backend, no hot path).

## Goal
In the create-link flow, a UTM builder that appends `utm_*` params to the destination URL, with reusable named templates. High value for marketing, low cost, enforces naming consistency.

## Decisions (locked, user delegated)
- Lives in `CreateLinkDialog.tsx` as an optional, collapsible "UTM parameters" section: inputs for `utm_source`, `utm_medium`, `utm_campaign`, `utm_term`, `utm_content`.
- A pure helper `applyUtm(url, params) -> string`: parses the URL (URL API), sets each non-empty utm_* query param (overwrites existing same-named), leaves the rest intact, returns the string. Invalid URL → return original unchanged (validation already happens on submit).
- A live preview of the final destination URL with the params applied.
- On submit, the link destination is the UTM-applied URL (the builder only transforms the `url` before it is sent; backend unchanged).
- **Templates (client-side):** save the current utm set under a name in `localStorage` (`quark.utmTemplates` = `{name: {source,medium,...}}`). A dropdown to apply a saved template into the fields, a "Save as template" action, and delete. Client-side is deliberate for a single-operator panel; server-side sync is a noted follow-up.
- All UI via i18n (EN + PT-BR). Code English, no inline `//`.

## Tasks (single implementer)
- `web/src/lib/utm.ts`: `applyUtm(url, params)` + template load/save/delete helpers over localStorage, with a typed `UtmParams`/`UtmTemplate`. Unit-tested (append to bare URL, URL with existing query, overwrite same param, empty params = no-op, invalid URL = unchanged; template round-trip).
- `web/src/components/CreateLinkDialog.tsx`: the collapsible UTM section + template dropdown/save/delete + live preview; applies UTM to `url` on submit.
- i18n `en.ts`/`pt-BR.ts`: a `utm` section.
- Vitest: `utm.ts` unit tests + a dialog test (filling utm fields changes the submitted url; applying a template fills the fields).
- README (both) + ROADMAP (both): one line for #8. No em-dashes.

## Constraints
- Frontend only; backend/redirect untouched. i18n parity EN/PT. Vitest+typecheck+lint+build green. No new dependency (URL API is built-in). Stay on feat/utm-builder; no merge.

## Out of scope
- Server-side template storage/sync (follow-up). UTM analytics attribution (the params live in the destination URL; click analytics already capture referrer).
