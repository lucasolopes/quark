import type { useT } from "@/i18n";
import { isHttpUrl } from "@/lib/codeguard";
import { durationToSeconds } from "@/lib/duration";

/** i18n namespace each dialog owns; picks `dialogs.create.*` vs `dialogs.edit.*` strings. */
export type LinkFormNamespace = "dialogs.create" | "dialogs.edit";

type TranslateFn = ReturnType<typeof useT>;

/** Fields the create and edit dialogs validate identically (modulo namespace). */
export interface LinkFormFields {
  url: string;
  ttl: string;
  ttlUnit: string;
  maxVisits: string;
  appIos: string;
  appAndroid: string;
  fallbackUrl: string;
  /** Edit-only: when the "remove expiry" box is checked, TTL validation is skipped. */
  removeExpiry?: boolean;
}

/** The subset of dialog form errors produced from the shared fields. */
export interface LinkFormErrors {
  url?: string;
  ttl?: string;
  maxVisits?: string;
  appIos?: string;
  appAndroid?: string;
  fallbackUrl?: string;
}

/**
 * Validate the fields shared by the create and edit link dialogs: destination
 * URL (required + http/https), optional TTL, max visits, app redirect targets,
 * and fallback URL. Each dialog layers its own extra checks (alias collision on
 * create, variant rows via `useVariantRows.validate`) on top of this result.
 */
export function validateLinkForm(
  fields: LinkFormFields,
  t: TranslateFn,
  ns: LinkFormNamespace,
): LinkFormErrors {
  const next: LinkFormErrors = {};
  if (!fields.url.trim()) {
    next.url = t(`${ns}.urlRequired`);
  } else if (!isHttpUrl(fields.url)) {
    next.url = t(`${ns}.urlInvalid`);
  }
  if (!fields.removeExpiry && fields.ttl.trim() && durationToSeconds(fields.ttl, fields.ttlUnit) == null) {
    next.ttl = t(`${ns}.ttlInvalid`);
  }
  const trimmedMaxVisits = fields.maxVisits.trim();
  if (trimmedMaxVisits) {
    const n = Number(trimmedMaxVisits);
    if (!Number.isInteger(n) || n <= 0) {
      next.maxVisits = t(`${ns}.maxVisitsInvalid`);
    }
  }
  if (fields.appIos.trim() && !isHttpUrl(fields.appIos)) {
    next.appIos = t(`${ns}.appDestInvalid`);
  }
  if (fields.appAndroid.trim() && !isHttpUrl(fields.appAndroid)) {
    next.appAndroid = t(`${ns}.appDestInvalid`);
  }
  if (fields.fallbackUrl.trim() && !isHttpUrl(fields.fallbackUrl)) {
    next.fallbackUrl = t(`${ns}.fallbackUrlInvalid`);
  }
  return next;
}
