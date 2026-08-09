# design-sync notes — quark

Repo-specific gotchas for future syncs. All code and comments in this repo MUST
be in English (user rule — applies to sync-authored files too).

- quark is an app, not a packaged design system. The DS surface is
  `web/src/components/` (shadcn-style `ui/` primitives + presentational app
  components) + the Tailwind 4 theme in `web/src/index.css`. Entry is the
  committed barrel `web/ds-entry.ts` (outside tsconfig.app include on purpose).
- No Storybook, no library `dist/` — package shape with `entry` pointing at the
  barrel; components enumerated in `componentSrcMap` (no shipped `.d.ts` tree,
  so the map is the authoritative component list).
- CSS is compiled one-shot with `@tailwindcss/cli` (see `buildCmd`) into
  `web/.ds-css/ds.css` (gitignored). `web/.ds-css/entry.css` (committed) adds an
  `@source` for `.design-sync/previews` so authored-preview utilities compile.
  Keep the CLI version in `buildCmd` in lockstep with `tailwindcss` in
  `web/package.json`.
- Fonts: `@fontsource-variable` packages declare only the "Variable" family
  names; the theme stacks also reference the base names. `extraFonts` includes
  `.design-sync/fonts-fallback.css` with base-name @font-face aliases —
  without it validate fires `[FONT_MISSING]`.
- `@/i18n` is a directory import; the converter's tsconfig-paths plugin
  resolves the dir before `/index.ts` and esbuild fails ("Incorrect
  function" on Windows). Fixed with `.design-sync/tsconfig.dsync.json` which
  maps the exact alias `@/i18n` → `web/src/i18n/index.ts` first. Any new
  directory-style alias import needs an exact entry there too.
- `[BUNDLE_EXPORT]` smoke failed because esbuild passes REGEX LITERALS through
  verbatim (strings are ASCII-escaped, regexes are not): the combining-marks
  range in `slugify` (CreateWorkspaceForm) shipped as raw UTF-8 and broke under
  the charset-less smoke page. Fixed in app source by using `\u0300-\u036f`
  escapes. If a future non-ASCII regex lands in any bundled source, the same
  failure returns — prefer escaped ranges.
- Render check: no playwright chromium cache on this machine; system Chrome is
  used via `DS_CHROMIUM_PATH="C:\Program Files\Google\Chrome\Application\chrome.exe"`.
  The `playwright` npm package is installed into `.ds-sync/` (scratch).
- Previews render dark-first: `PreviewProviders`
  (`.design-sync/preview-support.tsx`, wired via `extraEntries` + `provider`)
  adds the `dark` class on `<html>`, pins i18n to `en`, and wraps
  MemoryRouter + QueryClientProvider (fresh client, retry off).
- npm on this machine blocks postinstall scripts (`allow-scripts` warnings) —
  harmless for the sync deps.
- Preview authoring loop: whenever an authored preview adds NEW Tailwind
  utilities, re-run `buildCmd` (CSS recompile) BEFORE `package-build.mjs` —
  the compiled `ds.css` is a snapshot; stale CSS shows up as missing layout
  (e.g. `grid-cols-3` absent → stacked cells).
- Element screenshots have no page background; `PreviewProviders` paints the
  dark ink surface itself (`dark` wrapper div + bg-background). Don't remove it.

## Known render warns

- (none outstanding — GRID_OVERFLOW on Card/Input/Label/MeterBar/Skeleton/Tabs/
  StatCard remedied with `cardMode: column` overrides; Dialog/AlertDialog/
  DropdownMenu/Toaster/Terminal pinned to `cardMode: single` + viewport.)

## Re-sync risks

- `web/.ds-css/ds.css` is a compiled snapshot: any new Tailwind utility used in
  app code, previews, or `conventions.md` needs a `buildCmd` re-run before the
  converter, or layouts silently degrade. Keep `@tailwindcss/cli` version in
  `buildCmd` matched to `web/package.json`'s `tailwindcss`.
- The component list is hand-enumerated in `componentSrcMap` and mirrored by
  `web/ds-entry.ts`. A new component in `web/src/components` is NOT picked up
  automatically — add it to BOTH, and to `.design-sync/previews/` if it
  deserves a rich card.
- `conventions.md` enumerates real class names from the compiled CSS — re-run
  its validation grep after theme changes (`bg-*`/`text-*`/`font-*` tokens in
  `_ds_bundle.css`).
- Non-ASCII regex literals in any bundled source re-trigger the
  `[BUNDLE_EXPORT]` smoke failure (esbuild passes regex literals through raw).
- `DS_CHROMIUM_PATH` points at system Chrome; if Chrome updates break
  playwright's CDP handshake, install a pinned chromium instead
  (`cd .ds-sync && npx playwright install chromium`).
- Toaster preview fires `toast()` in an effect — if sonner's timing changes,
  the capture may race; check its sheet first on regressions.
- The upload plan/planId is session-scoped. Re-syncs go through the atomic
  path (`resync.mjs --remote` + fresh `finalize_plan`).
