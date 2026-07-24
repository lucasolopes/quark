# quark Design System

The visual language of **quark** — an open-source, single-binary URL shortener written in Rust ("the code is math, not a row in a database"). This system captures the product's fast/technical/cryptographic character: deep-ink surfaces, a single plasma-lime accent, and monospace as a first-class voice.

## Sources
- **GitHub:** `github.com/lucasolopes/quark` (Rust backend + `web/` React SPA). README, `src/permute.rs`, `src/codec.rs`, `src/main.rs` read as ground truth for content and claims.
- **Delivered design:** `Quark.dc.html` in this project (bilingual landing + admin-panel mock) — the seed this system was distilled from.
- **Official mark:** the *Feistel-crossing* glyph (`assets/quark-mark.svg` / `quark-tile.svg` / `quark-lockup.svg`), defined by this system from the engine itself — see Iconography. The source README uses shields.io badges + the 🦀 Rust mascot.

## CONTENT FUNDAMENTALS
- **Voice:** confident, technical, a little playful. States a bold claim, then proves it with a measured number. Never markety fluff.
- **Bilingual:** primary copy is Portuguese (BR); English is a peer, not an afterthought. Keep both when possible.
- **Person:** product-centric / imperative ("Suba o seu em um comando", "Ship yours in one command"). Rarely "we".
- **Honesty:** claims are hedged where the code hedges — e.g. "a não-enumerabilidade é uma propriedade estatística medida, não garantia criptográfica." Keep caveats; they build trust.
- **Numbers are the hero:** `~22M/s`, `18×`, `4 rounds`, `~1 MB`, `0 erros`. Always concrete, always measured; pair a numeral with a short qualifier.
- **Casing:** headings sentence-case. Eyebrows UPPERCASE mono with wide tracking ("COMO FUNCIONA", "BENCHMARKS"). Code/identifiers verbatim (`QUARK_KEY`, `GET /:code`).
- **Emoji:** essentially none, except the Rust 🦀 as a community wink in the footer. Don't decorate with emoji.
- **Examples:** "O código é matemática, não uma linha no banco." · "Rápido como um obfuscador, imprevisível como um cifrador, leve como um binário único."

## VISUAL FOUNDATIONS
- **Palette:** near-black **ink** (`#0A0B0F`) is the default ground; sections alternate with `#0C0D13`. Raised surfaces are **panel** `#131521` with a nested `#1A1D2B`. Exactly **one** brand color — **plasma lime** `#C6F94E` — used scarcely for the single primary action, active states, metric numerals and the glow. Secondary signal colors (cyan `#4ADEDE`, violet `#8B7CF6`, danger `#FF6B6B`) appear only in data/charts. Admin panel also ships a light theme.
- **Type:** three families with clear jobs — **Space Grotesk** (display/headings/metrics, tight `-0.03em`), **Hanken Grotesk** (body/UI), **JetBrains Mono** (codes, data, eyebrows, terminals). Mono is a real voice here, not just for code blocks.
- **Backgrounds:** flat ink. Two motifs only: soft radial accent/cyan glows behind the hero, and a faint 46px dot-grid with a radial mask. No photography, no illustration, no busy gradients.
- **Borders:** hairline `rgba(255,255,255,.09)`; strong `.16` for interactive outlines. Lines separate everything (tables, sections).
- **Radii:** 8/11/14/18px depending on element size; pills for toggles/badges chips. Cards are 14–18px.
- **Shadows:** near-none on the flat page; `--shadow-card`/`--shadow-modal` reserved for the terminal and modals. The one "shadow" that matters is the lime **glow** on the brand dot and active accents.
- **Cards:** panel background + hairline border + generous padding (22–26px); hover lifts `translateY(-3px)` and warms the border to `rgba(198,249,78,.3)`.
- **Motion:** restrained. Fades/upward rise on hero entrance (`qrise`), a slow pulse on the brand dot, `.15s`/`.3s` ease transitions on hover/press. No bounces.
- **Hover/press:** primary lime → brighter `#D8FF70` + slight lift; secondary → border brightens; bars animate width `.5s`. Press is a subtle brighten, not a big scale.
- **Transparency/blur:** sticky nav is `rgba(10,11,15,.72)` + `blur(14px)`; modals use a dark scrim + `blur(4px)`. Accent washes at 6–12% for active/hover fills.
- **Layout:** `1120px` max content, `40px` gutters, `80px` section rhythm. Grid + gap everywhere; monospace eyebrow → display heading → muted lead is the standard section head.

## ICONOGRAPHY
- The source ships **no icon font or SVG set**. Icons in the delivered design are inline **stroke SVGs** (~1.8–2px stroke, rounded caps) drawn in the accent or currentColor — feather/lucide-like. For new work, use **Lucide** as the closest match (SVG, 2px stroke, rounded).
- **No emoji** as UI icons (lone exception: 🦀 in the footer). The traffic-light dots (red/amber/green) are the terminal motif, not a general icon.
- **Brand mark (official):** the **Feistel-crossing** glyph — four lime nodes (the L/R halves in & out), an X crossing (the per-round swap) and a central ring (the ARX round-function). It encodes the actual engine (a reversible keyed permutation) and reads as a particle, echoing the name. Variants: `quark-mark.svg` (transparent), `quark-tile.svg` (ink rounded tile w/ glow — favicon/avatar), `quark-lockup.svg` (mark + Space Grotesk 700 wordmark). Pair with the wordmark; keep ≥8px clear space; lime on ink, or mark-only on lime. The earlier glowing particle-dot remains a valid secondary motif.

## Intentional additions
- **StatCard**, **MeterBar**, **Terminal** are not "generic" primitives but are core, repeated motifs in the quark product, so they're first-class components here.
