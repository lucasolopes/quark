# QR code enhancements — design (roadmap #2)

**Date:** 2026-07-14
**Branch:** `feat/qr-codes` (off main; no merge until reviewed)
**Effort:** low. Frontend only, entirely client-side (no API, no hot path).

## Goal

Extend the existing `LinkQrDialog` (which shows a QR and downloads a fixed
512px PNG at error-correction level M) to cover roadmap #2: PNG **and** SVG
download, a foreground/background **color** choice, and an **error-correction
level** selector. Generated in the SPA, never on the redirect hot path.

## What exists

`web/src/components/LinkQrDialog.tsx` uses `qrcode.react`'s `QRCodeSVG`
(`fgColor`/`bgColor`/`level`/`size` all supported by the lib, v4.2). PNG export
serializes the live SVG onto a temporary canvas on click. i18n keys under
`dialogs.qr`.

## Decisions (locked, user delegated)

- **Error correction:** a select with L / M / Q / H, default **M** (the current
  value). Feeds `QRCodeSVG level`.
- **Colors:** two color inputs, foreground default `#0A0B0F` (ink) and
  background default `#FFFFFF`. Feed `fgColor`/`bgColor`; the PNG export fills
  the background with the chosen `bgColor` (not hardcoded white). Add a small
  quiet-zone margin (`marginSize`) so scanners read it reliably.
- **SVG download:** a second download button. Serialize the live SVG (already
  behind `svgRef`) to a `Blob` (`image/svg+xml`) and download `quark-<code>.svg`.
  PNG download stays, honoring the chosen colors/level.
- Keep the dialog testable in jsdom (SVG serialization needs no canvas; PNG
  keeps the existing on-click canvas path).
- All new UI strings via i18n (EN source + PT-BR). Code English, no inline `//`.

## Components

- Modify `web/src/components/LinkQrDialog.tsx`: local state `level`, `fgColor`,
  `bgColor`; the controls (a `Select` for level, two `<input type="color">` with
  labels); pass them to `QRCodeSVG`; `handleDownloadPng` uses `bgColor`;
  new `handleDownloadSvg`.
- i18n: extend the `dialogs.qr` section in `en.ts` + `pt-BR.ts` (level label +
  the four level names or a short hint, foreground/background labels, "Download
  PNG" / "Download SVG").
- Tests (`web/src/components/LinkQrDialog.test.tsx`, new or extend): changing the
  level select updates the rendered QR's `level`; the SVG download click
  produces an anchor with `download="quark-<code>.svg"`; a non-default color is
  applied to the SVG (`fgColor`).

## Error handling / constraints

- Color inputs are constrained by the native picker; no validation needed.
- No network, no backend, no hot-path impact.
- Reused everywhere `LinkQrDialog` is already used (the Links table QR action);
  its props (`code`, `url`, `open`, `onOpenChange`) are unchanged, so callers
  need no edits.

## Out of scope

- Server-side QR endpoint (the roadmap explicitly says SPA-side; a `/qr`
  endpoint would only matter for programmatic use, deferrable).
- Logo-in-QR / custom shapes.
