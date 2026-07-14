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

export type PixelProvider = "ga4" | "meta_capi";

/**
 * Only the fields relevant to `provider` are populated. The backend masks
 * `api_secret`/`access_token` (returned as `••••` once a value is stored);
 * `measurement_id`/`pixel_id` come back in clear (they aren't secrets).
 */
export interface PixelCredentials {
  measurement_id?: string | null;
  api_secret?: string | null;
  pixel_id?: string | null;
  access_token?: string | null;
}
export interface Pixel {
  id: number;
  provider: PixelProvider;
  credentials: PixelCredentials;
  active: boolean;
  created: number;
}
export interface ListPixelsResponse { pixels: Pixel[]; }
export interface CreatePixelRequest {
  provider: PixelProvider;
  credentials: PixelCredentials;
  active?: boolean;
}
