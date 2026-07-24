/**
 * Turns a `per_*` breakdown map (country/device/browser/OS/referer/city) into
 * sorted, top-N data with the unknown-key fallback applied.
 *
 * Shared between `StatsCharts` (recharts bar/donut charts) and `StatsView`
 * (MeterBar rows for country/device/browser) so both surfaces rank and cap
 * breakdowns identically. Kept dependency-free (no React, no recharts) and
 * outside `StatsCharts.tsx` on purpose: `StatsCharts` is lazy-loaded to keep
 * recharts out of the main bundle, and `StatsView` needs this helper eagerly
 * — importing it from `StatsCharts.tsx` directly would drag recharts back
 * into the main chunk.
 */
export interface BreakdownDatum {
  label: string;
  count: number;
}

export function toBreakdownData(
  map: Record<string, number>,
  unknownLabel: string,
  topN?: number,
  relabel?: (label: string) => string,
): BreakdownDatum[] {
  const sorted = Object.entries(map)
    .toSorted(([, a], [, b]) => b - a)
    .map(([label, count]) => ({ label: label ? (relabel ? relabel(label) : label) : unknownLabel, count }));
  return topN === undefined ? sorted : sorted.slice(0, topN);
}

/** Countries are capped to the top N (long-tail lists get unwieldy); device/browser are shown in full. */
export const TOP_N_COUNTRIES = 8;
