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

/** One A/B destination: a URL and its relative weight (>= 1) in the weighted-random pick at redirect time. */
export interface Variant { url: string; weight: number; }
export interface Link {
  id: number;
  code: string;
  alias?: string;
  url: string;
  expiry: number | null;
  created: number;
  tags: string[];
  max_visits?: number;
  visits: number;
  rules: Rule[];
  variants: Variant[];
}
export interface ListLinksResponse { links: Link[]; next_after: number | null; }
export interface CreateLinkRequest { url: string; alias?: string; ttl?: number; tags?: string[]; max_visits?: number; rules?: Rule[]; variants?: Variant[]; }
export interface CreateLinkResponse { code: string; url: string; }
export interface TagsResponse { tags: string[]; }
export interface ClickEvent {
  id: number; ts: number;
  referer?: string | null; country?: string | null; user_agent?: string | null; city?: string | null;
  bot?: boolean;
}
export interface Aggregates {
  total: number; first_ts: number; last_ts: number;
  bots: number;
  per_day: Record<string, number>;
  per_country: Record<string, number>;
  per_device: Record<string, number>;
  per_os: Record<string, number>;
  per_browser: Record<string, number>;
  per_referer: Record<string, number>;
  per_city: Record<string, number>;
  /** Clicks per variant, keyed by the variant's index in `Link.variants` (as a string). */
  per_variant: Record<string, number>;
}
export interface Stats { aggregates: Aggregates; recent: ClickEvent[]; }
export interface BlocklistResponse { domains: string[]; }
export interface PatchLinkRequest { url?: string; ttl?: number | null; tags?: string[]; max_visits?: number | null; rules?: Rule[]; variants?: Variant[]; }

/** The 5 link lifecycle events a webhook subscription can be notified about. */
export const WEBHOOK_EVENTS = [
  "link.created",
  "link.updated",
  "link.deleted",
  "link.expired",
  "link.clicked",
] as const;
export type WebhookEvent = (typeof WEBHOOK_EVENTS)[number];

/** The channel a webhook subscription delivers to. `generic` signs with an HMAC secret; the others POST a channel-shaped payload straight to the pasted URL, unsigned. */
export const WEBHOOK_KINDS = ["generic", "slack", "discord", "telegram"] as const;
export type SubscriptionKind = (typeof WEBHOOK_KINDS)[number];

export interface Webhook {
  id: number;
  url: string;
  events: WebhookEvent[];
  active: boolean;
  created: number;
  kind: SubscriptionKind;
  /** Masked form of the signing secret, e.g. `whsec_••••` — the raw secret is only ever returned once, at creation. Empty for channel kinds (no signing secret). */
  secret_masked: string;
}
export interface ListWebhooksResponse { webhooks: Webhook[]; }
export interface CreateWebhookRequest { url: string; events: WebhookEvent[]; active?: boolean; kind: SubscriptionKind; }
export interface CreateWebhookResponse { id: number; secret: string; }
export interface PatchWebhookRequest { url?: string; events?: WebhookEvent[]; active?: boolean; }
export interface TestWebhookResponse { delivered: boolean; status: number; }
export interface ImportFailure { index: number; url: string; reason: string; }
export interface ImportSummary { imported: number; failed: ImportFailure[]; }
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
