# quark design system — conventions

quark is the admin panel of a URL shortener. **Dark-first identity**: deep-ink
surfaces with ONE plasma-lime accent used sparingly (primary action, active
state, hero numerals, focus ring). Light theme exists but dark is the brand.

## Setup and theming

- Components need **no provider to be styled** — tokens are plain CSS variables
  from `styles.css`. **Theme switching is the `dark` class on a root element**
  (`<html class="dark">` or a wrapper `<div className="dark bg-background text-foreground">`).
  Without `dark` you get the light theme; prefer dark for anything on-brand.
- Wrap the app in `I18nProvider` (exported) when using data-facing components
  (`LanguageSwitcher`, `TtlChips`, `LinkTable`, the Create/Edit dialogs) — they
  read strings from its context and crash outside it. Pure primitives (Button,
  Card, Input, …) don't need it.
- `PageHeader` with `back`, `MobileNav`, and `LinkTable` render router `<Link>`s —
  give them a react-router context (e.g. `MemoryRouter`) when used standalone.
- Toasts: mount `<Toaster position="bottom-right" />` once, then call the
  exported `toast.success("Link created", { description: "…" })` / `toast.error(…)`.

## Styling idiom — Tailwind 4 utilities with quark's token vocabulary

Style layout glue with Tailwind utility classes; colors and type ALWAYS go
through the theme tokens (never hardcode hex):

| Family | Classes |
|---|---|
| Surfaces | `bg-background` (page ink), `bg-card` (panel), `bg-popover`, `bg-secondary`, `bg-muted`, `bg-surface-input`, `bg-sidebar` |
| Text | `text-foreground`, `text-strong` (headings), `text-muted-foreground`, `text-brand-ink` (brand-colored text, contrast-safe in both themes), `text-destructive` |
| Accent (lime, use sparingly) | `bg-primary` + `text-primary-foreground` (filled action), `bg-accent-wash`, `border-accent-line`; focus rings come built into the primitives via the `--ring` token |
| Borders/hairlines | `border-border` (everything is hairline-bordered), `border-input` |
| Type | `font-sans` (Hanken Grotesk, body/UI), `font-heading` (Space Grotesk, display/headings/numerals), `font-mono` (JetBrains Mono, codes/data/eyebrow labels) |
| Type scale | `text-page-title` (27px display), `text-stat` (30px KPI numerals), `text-subtitle` (13.5px muted) + `tracking-display` on headings |
| Radius | scale derives from `--radius` (12px): `rounded-md` buttons/inputs ≈10 · `rounded-lg` cards 12 · `rounded-xl` modals ≈17 |
| Shadows | `shadow-card` (subtle), `shadow-modal` (deep modal drop) |
| Motion/motifs | `animate-rise` (page/modal entry), `animate-rise-out`, `card-hover` (lift on hover), `glow-glyph` (logo glow), `bg-hero-glow` + `bg-dot-grid` (out-of-shell backgrounds) |
| Charts | CSS vars `--chart-1`…`--chart-5` (lime, cyan, violet, danger, gray) |

Conventions the panel follows: mono uppercase eyebrows for section labels
(`font-mono text-xs uppercase tracking-wide text-muted-foreground`), mono for
short codes and metric values, `font-heading font-bold` for hero numerals,
hairline borders instead of heavy elevation.

## Compound components

`Card`, `Dialog`, `AlertDialog`, `DropdownMenu`, `Table`, `Tabs` are compound —
compose their exported subparts (`CardHeader`/`CardTitle`/`CardContent`/…,
`DialogContent`/`DialogHeader`/`DialogFooter`, `TableHeader`/`TableRow`/…).
Two gotchas: `DropdownMenuLabel` and `DropdownMenuItem` must sit inside a
`DropdownMenuGroup`; Button icons use `data-icon="inline-start"`/`"inline-end"`
on the icon element for correct padding. `TabsList` takes `variant="line"` for
the underline style.

## Where the truth lives

Read `styles.css` (imports `_ds_bundle.css` — the full compiled theme: token
definitions in `:root`/`.dark` plus every utility) and each component's
`components/<group>/<Name>/<Name>.prompt.md` + `<Name>.d.ts` for its API.

## Idiomatic example

```tsx
<div className="dark min-h-screen bg-background p-6 text-foreground">
  <PageHeader
    title="Links"
    subtitle="1,982 active links · 48,215 clicks in the last 30 days"
    actions={<Button><Plus data-icon="inline-start" /> New link</Button>}
  />
  <div className="grid grid-cols-3 gap-4">
    <StatCard value="48,215" label="Clicks · last 30 days" accent />
    <StatCard value="1,982" label="Active links" />
    <StatCard value="96.4%" label="Redirect uptime" />
  </div>
</div>
```

(`Plus` here stands for any 16px icon element; the panel uses lucide icons.)
