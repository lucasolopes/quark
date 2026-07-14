export interface Link {
  id: number;
  code: string;
  alias?: string;
  url: string;
  expiry: number | null;
  created: number;
}
export interface ListLinksResponse { links: Link[]; next_after: number | null; }
export interface CreateLinkRequest { url: string; alias?: string; ttl?: number; }
export interface CreateLinkResponse { code: string; url: string; }
export interface ClickEvent {
  id: number; ts: number;
  referer?: string | null; country?: string | null; user_agent?: string | null; city?: string | null;
}
export interface Aggregates {
  total: number; first_ts: number; last_ts: number;
  per_day: Record<string, number>;
  per_country: Record<string, number>;
  per_device: Record<string, number>;
  per_os: Record<string, number>;
  per_browser: Record<string, number>;
  per_referer: Record<string, number>;
  per_city: Record<string, number>;
}
export interface Stats { aggregates: Aggregates; recent: ClickEvent[]; }
export interface BlocklistResponse { domains: string[]; }
export interface PatchLinkRequest { url?: string; ttl?: number | null; }
