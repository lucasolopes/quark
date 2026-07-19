/**
 * Helpers for the A/B variant traffic split.
 *
 * Variants are stored as integer weights (`Variant.weight`), but the panel
 * presents them as percentages that must add up to 100. When the sum is 100
 * the weight is the percentage of traffic each destination receives.
 */

/**
 * Split 100% evenly across `count` variants. The remainder from the integer
 * division is handed to the first variants so the values always sum to exactly
 * 100 (e.g. 3 variants become 34/33/33).
 */
export function distributeEvenly(count: number): number[] {
  if (count <= 0) return [];
  const base = Math.floor(100 / count);
  const remainder = 100 - base * count;
  return Array.from({ length: count }, (_, i) => base + (i < remainder ? 1 : 0));
}

/**
 * Convert raw weights into percentages that sum to exactly 100, preserving the
 * relative proportions. Uses the largest-remainder method so rounding leftovers
 * go to the variants that lost the most. Legacy links stored non-normalized
 * weights (e.g. [1, 1]); this presents them as [50, 50]. A zero/empty total
 * falls back to an even split.
 */
export function normalizeToPercent(weights: number[]): number[] {
  const n = weights.length;
  if (n === 0) return [];
  const total = weights.reduce((s, w) => s + (w > 0 ? w : 0), 0);
  if (total <= 0) return distributeEvenly(n);
  const raw = weights.map((w) => ((w > 0 ? w : 0) / total) * 100);
  const floors = raw.map((r) => Math.floor(r));
  let leftover = 100 - floors.reduce((s, f) => s + f, 0);
  const order = raw
    .map((r, i) => ({ i, frac: r - Math.floor(r) }))
    .sort((a, b) => b.frac - a.frac);
  const result = [...floors];
  for (let k = 0; k < order.length && leftover > 0; k++, leftover--) {
    result[order[k].i] += 1;
  }
  return result;
}

/** Sum of the percentage strings, treating blank or non-numeric input as 0. */
export function variantsPercentTotal(weights: string[]): number {
  return weights.reduce((sum, w) => {
    const n = Number(w.trim());
    return sum + (Number.isFinite(n) ? n : 0);
  }, 0);
}
