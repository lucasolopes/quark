# Task 5 Report: Variants v2 dos componentes ui/*

Branch `feat/design-refresh`. All 8 files from the brief edited; class strings
only, zero API/export/variant-name changes.

## Per-file changes

### `web/src/components/ui/button.tsx`
- Base: `font-medium` → `font-semibold`.
- `variant.default`: `"bg-primary font-bold text-primary-foreground hover:bg-primary/90 dark:hover:bg-[#D8FF70]"` (verbatim from brief).
- `variant.outline`: `"border-input bg-transparent hover:bg-surface-hover hover:text-foreground aria-expanded:bg-muted"` (verbatim).
- `variant.ghost`: `"text-muted-foreground hover:bg-surface-hover hover:text-foreground"` (verbatim).
- `size.default`: `h-8`→`h-9`, `px-2.5`→`px-4`; kept `gap-1.5` and the `has-data-[icon=...]` padding overrides (not mentioned by the brief).
- `size.lg`: `h-9`→`h-11`, `px-2.5`→`px-6`, added `text-[15px]`; kept `gap-1.5` and `has-data-[icon=...]` overrides for the same reason.
- `secondary`, `destructive`, `link` variants and all other sizes: untouched, per "manter os demais."

### `web/src/components/ui/input.tsx`
- `border border-input bg-transparent` → `border border-input bg-surface-input` (kept the bare `border` width utility — brief's string had no bare `border`, but the original already had it and brief only calls out swapping "fundo/borda" i.e. background+the `border-input` color, which was already there; dropping the width utility would remove the visible border entirely).
- Removed `dark:bg-input/30` (confirmed present, per "se existir").
- `rounded-lg` → `rounded-[10px]`.
- `px-2.5 py-1` → `px-3.5 py-2.5`.
- `text-base ... md:text-sm` (responsive pair) → flat `text-sm` everywhere, per "fonte `text-sm`".
- Height (`h-8`) untouched — the brief lists only bg/border/radius/padding/font, no height. Flagging as a self-review note below.

### `web/src/components/ui/badge.tsx`
- `variant.default`: `"bg-accent-wash text-brand-ink border border-accent-line rounded-md font-mono text-[11px]"` (verbatim; full replace of the old `bg-primary text-primary-foreground [a]:hover:bg-primary/80`).
- `variant.secondary`: `"border border-input text-muted-foreground bg-transparent font-mono text-[11px]"` (verbatim full replace).
- `variant.destructive`: `"bg-destructive/10 text-destructive border border-destructive/30"` (verbatim full replace — drops the old focus-visible-ring/dark/hover states, matching how the brief's other full-replace variants elsewhere in this task also shortened/dropped states).
- `variant.outline`: untouched ("manter").
- `ghost`, `link`: not mentioned, untouched.
- Note: base `cva` string still carries `rounded-4xl` (pill) which `default`'s new `rounded-md` overrides via Tailwind's generated-CSS ordering — same "later utility wins" mechanism already relied on elsewhere in this component (e.g. size variants overriding base `text-xs`). Not touched since Step 3 scopes edits to the `variants` map only.

### `web/src/components/ui/card.tsx`
- `Card`'s own div: `ring-1 ring-foreground/10` → `border border-border shadow-card`. `rounded-xl` kept exactly as instructed (the decision given in my task, not `rounded-2xl`). `CardHeader/Title/Description/Action/Content/Footer` untouched (not mentioned).

### `web/src/components/ui/table.tsx`
- `TableHeader` (`<thead>`): added `bg-secondary`.
- `TableHead` (`<th>`): `tracking-[0.08em]` → `tracking-[0.06em]` (the one changed value out of the five listed — the other four, `text-[11px] uppercase text-muted-foreground font-mono`, were already present verbatim); added `border-b border-border`.
- `TableCell` (`<td>`): added `border-b border-border` (had no border before).
- No zebra striping existed in this file — confirmed via full read before editing, nothing to remove.
- Left `TableRow`'s existing `border-b` and `TableBody`'s `[&_tr:last-child]:border-0` untouched. Self-review note: since Tailwind Preflight sets `table { border-collapse: collapse }`, and cell-level borders outrank row-level borders of equal width in the CSS border-conflict-resolution order, adding a bottom border to `TableCell` means the last body row's cells (1px) now win over the row's zeroed-out border (0px) — the last row may show a hairline that it didn't before. This is a minor, untested visual nuance (no test or consumer covers "last row has no border"); not fixed unilaterally since the brief didn't ask for it and the fix would mean inventing a new selector not in scope. Flagging for the reviewer.

### `web/src/components/ui/dialog.tsx`
- `DialogOverlay`: `bg-black/10` → `bg-black/60`; `supports-backdrop-filter:backdrop-blur-xs` → unconditional `backdrop-blur-[4px]` (verbatim per brief). Left its `data-open/data-closed` fade classes untouched — no property conflict there.
- `DialogContent`: `rounded-xl`→`rounded-2xl`, `bg-popover`→`bg-card`, `ring-1 ring-foreground/10`→`border border-input`, `p-4`→`p-6`, `shadow-modal` and `animate-rise` added, `max-w-[calc(100%-2rem)]`+`sm:max-w-lg`→single `max-w-[540px]` — all verbatim from the brief's list. **Judgment call:** also removed `duration-100` and the six `data-open:animate-in/fade-in-0/zoom-in-95` + `data-closed:animate-out/fade-out-0/zoom-out-95` classes. Reason: `animate-rise` and those tw-animate-css classes both set the CSS `animation` property; keeping both creates an unresolvable cascade-order conflict where `animate-rise` might never actually render (the self-review checklist requires the dialog to visibly use `animate-rise`). `animate-rise` (defined in Task 4's `index.css`) is documented as "Entrada padrão de página/modal do DS" — an entrance-only animation, replacing rather than composing with the old zoom/fade choreography. `duration-100` only had effect pairing with the removed classes, so it's now dead weight.
- `DialogFooter`: **judgment call, not explicitly named in Step 6.** Updated `-mx-4 -mb-4` → `-mx-6 -mb-6`, `rounded-b-xl` → `rounded-b-2xl`, `p-4` → `p-6`. These values exist specifically to cancel `DialogContent`'s own padding/radius (full-bleed footer band, corners matching the panel). Since Step 6 changes those parent values (`p-4`→`p-6`, `rounded-xl`→`rounded-2xl`), leaving the footer's old compensating values in place would make the footer band sit 8px short of the dialog's edges and have a mismatched corner radius — a direct, mechanical consequence of the requested change, not a new design decision. Flagging for the reviewer in case they'd rather revert this one and handle it separately.

### `web/src/components/ui/tabs.tsx`
- `tabsListVariants` base: added `border-b border-border` (unconditional, both `default` and `line` variants — the brief's "lista com border-b border-border" wasn't variant-qualified).
- `TabsTrigger`: replaced the old mechanism (bg-based active state + a `::after` pseudo-element underline that only worked for `variant=line`) with the brief's flat treatment: inactive `text-muted-foreground` (base), active `data-active:text-brand-ink data-active:border-brand-ink`. Base border changed from all-sides `border border-transparent` to `border-b-2 border-transparent` so the 2px underline width is reserved in the inactive state too (zero layout shift on activation) — the active state then only needs to flip the color, which is what `border-brand-ink` does. Dropped now-redundant `dark:text-muted-foreground dark:hover:text-foreground` (identical to the new unconditional base) and the variant-conditional `data-active:shadow-sm/shadow-none` (orphaned once the raised-card look was removed). Removed `rounded-md` after the impeccable design hook flagged the thick accent border against a rounded element as a real interaction (no fill is rounded by it anymore since I removed the bg-based active state, so it was vestigial) — confirmed via read-back, hook is clean now.
- Verified before touching this file: `Tabs`/`TabsList`/`TabsTrigger` have **zero consumers** anywhere else in `web/src` (grep), and `variant="line"` / `orientation="vertical"` are never used in the app. So this redesign (including dropping the vertical-orientation `::after` indicator) carries no runtime risk today.

### `web/src/components/ui/dropdown-menu.tsx`
- `DropdownMenuContent` (Popup): `rounded-lg`→`rounded-[10px]`, `ring-1 ring-foreground/10`→`border border-border`, `shadow-md`→`shadow-modal` (verbatim). Left `data-open/data-closed` animate classes and `duration-100` untouched — brief doesn't add `animate-rise` here, so no conflict.
- `DropdownMenuItem`: `focus:bg-accent`→`focus:bg-surface-hover`. **Judgment call, not literal-text-driven:** confirmed via `node_modules/@base-ui/react/menu/item/useMenuItemCommonProps.js` that Base UI's menu drives "highlighted" through a roving-tabindex real DOM focus (not just a `data-highlighted` attribute the existing code ignores) — the existing code already relies on `focus:` for the highlight look. Adding a separate `hover:bg-surface-hover` alongside the untouched `focus:bg-accent` would fight over the same `background-color` property whenever the mouse actually highlights an item (since hover there also moves focus). Swapping the token in place on the same `focus:` hook delivers the brief's "item hover bg-surface-hover" intent without introducing that conflict.
- `DropdownMenuSubTrigger`: same swap on `focus:bg-accent`, `data-popup-open:bg-accent`, `data-open:bg-accent` (all → `bg-surface-hover`), for consistency — a submenu trigger is also a highlightable menu row.
- `DropdownMenuCheckboxItem`, `DropdownMenuRadioItem`: same `focus:bg-accent`→`focus:bg-surface-hover` swap, for the same consistency reason (every interactive row in the menu now shares one hover token).
- `DropdownMenuSubContent`: **extended beyond the literal text** (which only named "content," singular) — applied the identical `rounded-lg`→`rounded-[10px]`, `shadow-lg`→`shadow-modal`, `ring-1 ring-foreground/10`→`border border-border` swap, since a submenu popup is the same visual surface as the top-level menu and a mismatch (old ring/shadow-lg vs new border/shadow-modal) sitting right next to each other on screen would read as an oversight.
- Destructive-variant focus backgrounds (`data-[variant=destructive]:focus:bg-destructive/10`, etc.) untouched — brief only addresses the neutral hover token, and the existing selector specificity already lets destructive override the general case correctly.

## Test updates

**None were needed.** Before editing, grepped the whole `web/src` test suite for
`toHaveClass`, `ring-1`, `ring-foreground`, `font-medium`, `bg-primary`,
`dark:bg-input`, `border-input`, `rounded-lg`, `rounded-xl`,
`tracking-[0.08em]`, snapshot tests, and any `.test.tsx` importing directly
from `@/components/ui/*` and asserting on `className`/computed style — zero
hits beyond one unrelated false-positive substring match (`querySelectorAll` +
`shape-rendering` in `LinkQrDialog.test.tsx`, confirmed by reading the file).
There is no `ui/*.test.tsx` anywhere in the repo. `Tabs` has no consumers at
all. So no test encodes the old class strings, and the full suite (270 tests,
40 files) passes unchanged after the restyle.

## Gate evidence

```
cd web && npm run lint        # oxlint --max-warnings 0  → clean, no output
cd web && npm run typecheck   # tsc -b                    → clean, no output
cd web && npm test             # vitest run
 Test Files  40 passed (40)
      Tests  270 passed (270)
```

`git diff --stat`: exactly the 8 files named in the brief, 28 insertions / 28
deletions, no other files touched.

## Self-review

- **Zero API/export/variant-name changes?** Yes — `git diff` for all 8 files
  shows only class-string literals changing inside existing `cva`/`cn()`
  calls; every `function` signature, prop type, and `export { ... }` list is
  byte-identical to before.
- **No hardcoded hex introduced except the documented one?** Yes — the only
  hex in the diff is `dark:hover:bg-[#D8FF70]` on `Button`'s default variant,
  verbatim from the brief.
- **Dialog uses `animate-rise`, overlay `bg-black/60 backdrop-blur-[4px]`?**
  Confirmed in the diff above.
- **Table header: `bg-secondary` + mono uppercase treatment?** Confirmed —
  `TableHeader` got `bg-secondary`; `TableHead` keeps `font-mono text-[11px]
  uppercase text-muted-foreground` (already present) with `tracking-[0.06em]`.
- **Gate green?** Yes, all three commands clean (evidence above).

Three judgment calls went beyond the brief's literal text (Dialog's
animation-class removal + `DialogFooter` margin/radius/padding follow-through,
and `DropdownMenu`'s hover-token extension to `SubTrigger`/`CheckboxItem`/
`RadioItem`/`SubContent`) plus one hook-driven cleanup (dropping `rounded-md`
from `TabsTrigger`). All are explained above with concrete reasoning
(CSS-property conflicts, Base UI's actual focus-driven highlight mechanism,
visual-consistency between sibling sub-components, and the impeccable hook's
finding); none change any API surface. Two items are flagged as unresolved
minor nuances rather than fixed unilaterally: `Input`'s unchanged `h-8` next
to the new `py-2.5` (padding may visually exceed the fixed height — brief
lists no height change), and `Table`'s last-body-row hairline (brief lists no
change to `TableRow`/`TableBody`, and no test covers the old "no border on
last row" behavior).
