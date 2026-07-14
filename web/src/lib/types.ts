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
export interface PatchLinkRequest { url?: string; ttl?: number | null; }

export type Scope = "links_read" | "links_write" | "blocklist" | "webhooks" | "analytics" | "full";
export const ALL_SCOPES: Scope[] = ["links_read", "links_write", "blocklist", "webhooks", "analytics", "full"];

export interface ApiToken {
  id: number;
  name: string;
  scopes: Scope[];
  rate_limit_per_min: number | null;
  created: number;
}
export interface ListTokensResponse { tokens: ApiToken[]; }
export interface CreateTokenRequest { name: string; scopes: Scope[]; rate_limit_per_min?: number; }
export interface CreateTokenResponse { id: number; token: string; }
