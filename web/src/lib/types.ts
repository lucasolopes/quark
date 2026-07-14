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
