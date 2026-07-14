/** The visitor attribute a `Rule` matches on (roadmap #12). OS/browser rules are out of scope. */
export type RuleField = "country" | "device";

/**
 * A single geo/device redirect rule: if the visitor's `field` value is in
 * `values`, the redirect goes to `to` instead of the link's default `url`.
 * Evaluated in order, first match wins (see `docs/REDIRECT-RULES.md`).
 */
export interface Rule {
  field: RuleField;
  values: string[];
  to: string;
}

export interface Link {
  id: number;
  code: string;
  alias?: string;
  url: string;
  expiry: number | null;
  created: number;
  rules: Rule[];
}
export interface ListLinksResponse { links: Link[]; next_after: number | null; }
export interface CreateLinkRequest { url: string; alias?: string; ttl?: number; rules?: Rule[]; }
export interface CreateLinkResponse { code: string; url: string; }
export interface ClickEvent {
  id: number; ts: number;
  referer?: string | null; country?: string | null; user_agent?: string | null;
}
export interface Aggregates {
  total: number; first_ts: number; last_ts: number;
  per_day: Record<string, number>;
  per_country: Record<string, number>;
  per_device: Record<string, number>;
}
export interface Stats { aggregates: Aggregates; recent: ClickEvent[]; }
export interface BlocklistResponse { domains: string[]; }
export interface PatchLinkRequest { url?: string; ttl?: number | null; rules?: Rule[]; }
