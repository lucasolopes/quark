/**
 * Shared date formatting for the Links and Stats screens.
 * Epoch in SECONDS (as returned by the API) — converted to milliseconds
 * before passing to Date/Intl. `0`/`null`/`undefined` mean "no value" (not
 * a real zero epoch) in API responses, hence the guard.
 */

/** Short date (day/month/year), pt-BR. `formatDate(0)` -> "—". */
export function formatDate(epochSeconds: number): string {
  if (!epochSeconds) return "—";
  return new Date(epochSeconds * 1000).toLocaleDateString("pt-BR", {
    day: "2-digit",
    month: "2-digit",
    year: "numeric",
  });
}

/** Date and time (day/month/year hour:minute), pt-BR. `formatDateTime(0)` -> "—". */
export function formatDateTime(epochSeconds: number): string {
  if (!epochSeconds) return "—";
  return new Date(epochSeconds * 1000).toLocaleString("pt-BR", {
    day: "2-digit",
    month: "2-digit",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}
