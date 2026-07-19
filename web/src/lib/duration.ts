/**
 * Duration helpers for link expiration (`ttl`). The API takes a TTL in seconds,
 * but the panel lets the user pick a unit (minutes, hours, days, weeks, months)
 * and type a plain value, which is far friendlier than raw seconds.
 *
 * A month is treated as 30 days, matching the coarse "expires in N months"
 * intent (the API does not need calendar precision for a TTL).
 */
export type DurationUnitKey = "minutes" | "hours" | "days" | "weeks" | "months";

export const DURATION_UNITS: { key: DurationUnitKey; seconds: number }[] = [
  { key: "minutes", seconds: 60 },
  { key: "hours", seconds: 3600 },
  { key: "days", seconds: 86400 },
  { key: "weeks", seconds: 604800 },
  { key: "months", seconds: 2592000 },
];

/** Default unit for a fresh expiration field. */
export const DEFAULT_DURATION_UNIT: DurationUnitKey = "days";

function unitSeconds(key: string): number {
  return DURATION_UNITS.find((u) => u.key === key)?.seconds ?? 1;
}

/**
 * Convert a value + unit into seconds. Returns null when the value is blank or
 * not a whole number greater than zero, so callers can flag it as invalid.
 */
export function durationToSeconds(value: string, unitKey: string): number | null {
  const trimmed = value.trim();
  if (!trimmed) return null;
  const n = Number(trimmed);
  if (!Number.isInteger(n) || n <= 0) return null;
  return n * unitSeconds(unitKey);
}
