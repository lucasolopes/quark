/** UTM parameters, one optional field per `utm_*` query param. */
export interface UtmParams {
  source?: string;
  medium?: string;
  campaign?: string;
  term?: string;
  content?: string;
}

/** A named, saved set of UTM params. */
export interface UtmTemplate {
  name: string;
  params: UtmParams;
}

const UTM_KEYS: readonly (keyof UtmParams)[] = ["source", "medium", "campaign", "term", "content"];

const TEMPLATES_STORAGE_KEY = "quark.utmTemplates";

type TemplateMap = Record<string, UtmParams>;

/**
 * Applies every non-empty `utm_*` param onto `url`'s query string, overwriting
 * any same-named param already present and leaving the rest of the URL intact.
 * If `url` cannot be parsed (invalid URL), it is returned unchanged — validation
 * of the URL itself already happens elsewhere before submit.
 */
export function applyUtm(url: string, params: UtmParams): string {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return url;
  }
  for (const key of UTM_KEYS) {
    const value = params[key]?.trim();
    if (value) {
      parsed.searchParams.set(`utm_${key}`, value);
    }
  }
  return parsed.toString();
}

function readTemplates(): TemplateMap {
  try {
    if (typeof localStorage === "undefined") return {};
    const raw = localStorage.getItem(TEMPLATES_STORAGE_KEY);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
      return parsed as TemplateMap;
    }
    return {};
  } catch {
    return {};
  }
}

function writeTemplates(templates: TemplateMap): void {
  try {
    if (typeof localStorage === "undefined") return;
    localStorage.setItem(TEMPLATES_STORAGE_KEY, JSON.stringify(templates));
  } catch {
    /** localStorage may be unavailable (quota, private mode, disabled) — no-op. */
  }
}

/** Loads all saved UTM templates, keyed by name. Tolerates a missing or corrupted store. */
export function loadUtmTemplates(): TemplateMap {
  return readTemplates();
}

/** Saves (or overwrites, if the name already exists) a named UTM template. */
export function saveUtmTemplate(name: string, params: UtmParams): void {
  const templates = readTemplates();
  templates[name] = params;
  writeTemplates(templates);
}

/** Deletes a named UTM template. No-op if it doesn't exist. */
export function deleteUtmTemplate(name: string): void {
  const templates = readTemplates();
  delete templates[name];
  writeTemplates(templates);
}
