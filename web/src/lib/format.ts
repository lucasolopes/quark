/**
 * Shared date/number formatting for the panel.
 * Epoch in SECONDS (as returned by the API) — converted to milliseconds
 * before passing to Date/Intl. `0`/`null`/`undefined` mean "no value" (not
 * a real zero epoch) in API responses, hence the guard.
 *
 * Formatting follows the active UI locale (read from the same store the i18n
 * provider persists to). The provider writes the choice synchronously before
 * re-rendering consumers, so a render after a locale switch picks up the new
 * locale here too. Falls back to pt-BR when unset.
 */
import { STORAGE_KEY } from "@/i18n/shared";

function activeLocale(): string {
  if (typeof localStorage !== "undefined") {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored === "en" || stored === "pt-BR") return stored;
  }
  return "pt-BR";
}

/** Short date (day/month/year) in the active locale. `formatDate(0)` -> "—". */
export function formatDate(epochSeconds: number): string {
  if (!epochSeconds) return "—";
  return new Date(epochSeconds * 1000).toLocaleDateString(activeLocale(), {
    day: "2-digit",
    month: "2-digit",
    year: "numeric",
  });
}

/** Date and time (day/month/year hour:minute) in the active locale. `formatDateTime(0)` -> "—". */
export function formatDateTime(epochSeconds: number): string {
  if (!epochSeconds) return "—";
  return new Date(epochSeconds * 1000).toLocaleString(activeLocale(), {
    day: "2-digit",
    month: "2-digit",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** Integer with the active locale's grouping (e.g. 1.234 / 1,234). */
export function formatNumber(value: number): string {
  return value.toLocaleString(activeLocale());
}
