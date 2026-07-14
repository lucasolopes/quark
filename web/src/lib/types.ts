export interface Link {
  id: number;
  code: string;
  alias?: string;
  url: string;
  app_ios?: string;
  app_android?: string;
  expiry: number | null;
  created: number;
}
export interface ListLinksResponse { links: Link[]; next_after: number | null; }
export interface CreateLinkRequest { url: string; alias?: string; ttl?: number; app_ios?: string; app_android?: string; }
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
export interface PatchLinkRequest { url?: string; ttl?: number | null; app_ios?: string | null; app_android?: string | null; }

/** The two well-known app-association document names the backend accepts. */
export type WellknownName = "apple-app-site-association" | "assetlinks.json";
