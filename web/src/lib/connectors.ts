import { usePixels, useSheetsStatus, useWebhooks } from "@/lib/queries";
import type { MessageKey } from "@/i18n";
import type { PixelProvider, SubscriptionKind, WebhookEvent } from "@/lib/types";

/**
 * Which real quark feature powers an integration. `sheets` is the one native
 * OAuth connector (its own connect/sync/disconnect flow); `soon` is not built
 * yet.
 */
export type PoweredBy = "webhooks" | "pixels" | "sheets" | "soon";

export type Category = "notifications" | "automation" | "analytics" | "devData";

export interface Integration {
  id: string;
  name: string;
  /** Short monogram shown inside the brand-colored badge (e.g. "Sl"). */
  mono: string;
  /** Brand color, used ONLY as the mono badge fill. */
  color: string;
  descKey: MessageKey;
  category: Category;
  poweredBy: PoweredBy;
}

/**
 * Curated catalog. Each connectable item opens its own dedicated view
 * (`/extensions/:id`) that either connects over OAuth (Sheets) or takes the
 * credentials/URL a driver needs (webhooks, pixels). `soon` items are not
 * built yet and are shown as read-only "coming soon" cards.
 */
export const INTEGRATIONS: Integration[] = [
  // Notifications — powered by Webhooks.
  { id: "slack", name: "Slack", mono: "Sl", color: "#4A154B", descKey: "extensions.slackDesc", category: "notifications", poweredBy: "webhooks" },
  { id: "discord", name: "Discord", mono: "D", color: "#5865F2", descKey: "extensions.discordDesc", category: "notifications", poweredBy: "webhooks" },
  { id: "telegram", name: "Telegram", mono: "T", color: "#26A5E4", descKey: "extensions.telegramDesc", category: "notifications", poweredBy: "webhooks" },
  // Automation — powered by Webhooks.
  { id: "zapier", name: "Zapier", mono: "Z", color: "#FF4A00", descKey: "extensions.zapierDesc", category: "automation", poweredBy: "webhooks" },
  { id: "make", name: "Make", mono: "M", color: "#6D00CC", descKey: "extensions.makeDesc", category: "automation", poweredBy: "webhooks" },
  { id: "n8n", name: "n8n", mono: "n8", color: "#EA4B71", descKey: "extensions.n8nDesc", category: "automation", poweredBy: "webhooks" },
  { id: "sheets", name: "Google Sheets", mono: "GS", color: "#0F9D58", descKey: "extensions.sheetsDesc", category: "automation", poweredBy: "sheets" },
  // Analytics — GA4 and Meta powered by Pixels; the rest not built yet.
  { id: "ga4", name: "GA4 Measurement", mono: "GA", color: "#E37400", descKey: "extensions.ga4Desc", category: "analytics", poweredBy: "pixels" },
  { id: "meta", name: "Meta CAPI", mono: "f", color: "#0866FF", descKey: "extensions.metaDesc", category: "analytics", poweredBy: "pixels" },
  { id: "gtm", name: "Tag Manager", mono: "GTM", color: "#246FDB", descKey: "extensions.gtmDesc", category: "analytics", poweredBy: "soon" },
  { id: "tiktok", name: "TikTok Events", mono: "TT", color: "#111318", descKey: "extensions.tiktokDesc", category: "analytics", poweredBy: "soon" },
  { id: "linkedin", name: "LinkedIn CAPI", mono: "in", color: "#0A66C2", descKey: "extensions.linkedinDesc", category: "analytics", poweredBy: "soon" },
  // Dev & Data — not built yet.
  { id: "notion", name: "Notion", mono: "N", color: "#111318", descKey: "extensions.notionDesc", category: "devData", poweredBy: "soon" },
];

/** Render order of the category groups, with their eyebrow label keys. */
export const CATEGORY_ORDER: { category: Category; labelKey: MessageKey }[] = [
  { category: "automation", labelKey: "extensions.categoryAutomation" },
  { category: "notifications", labelKey: "extensions.categoryNotifications" },
  { category: "analytics", labelKey: "extensions.categoryAnalytics" },
  { category: "devData", labelKey: "extensions.categoryDevData" },
];

/** Maps each webhook event to its i18n label key (reused from the Webhooks screen). */
export const EVENT_LABEL_KEY: Record<WebhookEvent, MessageKey> = {
  "link.created": "webhooks.eventCreated",
  "link.updated": "webhooks.eventUpdated",
  "link.deleted": "webhooks.eventDeleted",
  "link.expired": "webhooks.eventExpired",
  "link.clicked": "webhooks.eventClicked",
  "link.threshold_reached": "webhooks.eventThresholdReached",
};

/**
 * Fixed webhook `kind` per integration id (aligned with LUC-15: no kind
 * selector in the UI). Native channels sign nothing and POST a channel-shaped
 * payload; the automation tools (Zapier/Make/n8n) are `generic` and get an
 * HMAC signing secret shown once.
 */
export const WEBHOOK_KIND_BY_ID: Record<string, SubscriptionKind> = {
  slack: "slack",
  discord: "discord",
  telegram: "telegram",
  zapier: "generic",
  make: "generic",
  n8n: "generic",
};

/** Fixed pixel provider per integration id. */
export const PIXEL_PROVIDER_BY_ID: Record<string, PixelProvider> = {
  ga4: "ga4",
  meta: "meta_capi",
};

/** Looks up a catalog entry by id (the `/extensions/:id` route param). */
export function getIntegration(id: string | undefined): Integration | undefined {
  return INTEGRATIONS.find((i) => i.id === id);
}

/**
 * Per-connector connection status derived from the existing feature APIs
 * (LUC-87 fase 1): a connector is "connected" when its backing resource
 * exists. Limitation: the generic-webhook connectors (Zapier/Make/n8n) share
 * `kind: "generic"`, so they cannot be told apart until the connection model
 * stores a connector id (fase 3) — they light up together once any generic
 * webhook exists.
 */
export function useConnectedIds(): Set<string> {
  const webhooks = useWebhooks();
  const pixels = usePixels();
  const sheets = useSheetsStatus();
  const connected = new Set<string>();
  const webhookKinds = new Set((webhooks.data?.webhooks ?? []).map((w) => w.kind));
  const pixelProviders = new Set((pixels.data?.pixels ?? []).map((p) => p.provider));
  for (const it of INTEGRATIONS) {
    if (it.poweredBy === "webhooks" && webhookKinds.has(WEBHOOK_KIND_BY_ID[it.id])) connected.add(it.id);
    if (it.poweredBy === "pixels" && pixelProviders.has(PIXEL_PROVIDER_BY_ID[it.id])) connected.add(it.id);
  }
  if (sheets.data?.connected) connected.add("sheets");
  return connected;
}
