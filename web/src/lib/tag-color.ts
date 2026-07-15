/**
 * Deterministic per-tag color, computed purely on the frontend (the backend
 * stores no color). The same tag name always maps to the same swatch, and the
 * palette is drawn from the brand chart tokens so chips sit inside the product's
 * visual register instead of reading as a rainbow.
 *
 * The primary lime (`--chart-1` / `--primary`) is deliberately excluded — it's
 * the brand action color and stays scarce.
 */

/** A resolved tag swatch. Values are CSS colors ready to drop into `style`. */
export interface TagColor {
  /** Solid hue, for the leading dot. */
  dot: string;
  /** Readable text color: the hue blended toward the theme foreground so it
   * darkens in light mode and lightens in dark mode. */
  text: string;
  /** Subtle tint for the chip background (hue at low alpha over any surface). */
  bg: string;
}

/**
 * Base hues, keyed to the brand chart palette:
 * - cyan   `--chart-2` (#4ADEDE)
 * - violet `--chart-3` (#8B7CF6)
 * plus amber, rose and teal to widen the set without touching the lime action.
 */
const PALETTE = [
  "#4ADEDE", // cyan
  "#8B7CF6", // violet
  "#FEBC2E", // amber
  "#FB7185", // rose
  "#2DD4BF", // teal
] as const;

/**
 * FNV-1a over the UTF-16 code units — a small, stable string hash. Stable across
 * runs (no randomness), so a tag keeps its color between sessions and machines.
 */
function hash(name: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < name.length; i++) {
    h ^= name.charCodeAt(i);
    // FNV prime, kept in 32-bit range via Math.imul.
    h = Math.imul(h, 0x01000193);
  }
  return h >>> 0;
}

/** Maps a tag name to a stable swatch from the brand palette. */
export function tagColor(name: string): TagColor {
  const hue = PALETTE[hash(name) % PALETTE.length];
  return {
    dot: hue,
    // Blend toward `--foreground` so the label keeps contrast in both themes:
    // dark foreground (light mode) darkens the hue, light foreground (dark
    // mode) lightens it.
    text: `color-mix(in srgb, ${hue} 60%, var(--foreground))`,
    // Low-alpha tint reads as a soft chip over either the light or dark surface.
    bg: `color-mix(in srgb, ${hue} 14%, transparent)`,
  };
}
