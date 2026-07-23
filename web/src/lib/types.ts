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
/**
 * A per-link click-threshold alert rule (LUC-38/LUC-66): fire
 * `link.threshold_reached` when the link is clicked at least `threshold`
 * times within a fixed window of `window_secs` seconds.
 */
export interface AlertRule { threshold: number; window_secs: number; }
/** Destination health from the background checker; absent when never probed. */
export interface LinkHealth { healthy: boolean; status?: number; checked_at: number; }
export interface Link {
  id: number;
  code: string;
  alias?: string;
  url: string;
  app_ios?: string;
  app_android?: string;
  folder?: string;
  fallback_url?: string;
  has_password?: boolean;
  health?: LinkHealth;
  expiry: number | null;
  created: number;
  tags: string[];
  max_visits?: number;
  visits: number;
  rules: Rule[];
  variants: Variant[];
}
export interface ListLinksResponse { links: Link[]; next_after: number | null; }
export interface CreateLinkRequest { url: string; alias?: string; ttl?: number; tags?: string[]; max_visits?: number; rules?: Rule[]; variants?: Variant[]; app_ios?: string; app_android?: string; folder?: string; fallback_url?: string; password?: string; }
export interface CreateLinkResponse { code: string; url: string; }
/** A tag in use by at least one link, with how many links carry it. */
export interface Tag { name: string; count: number; }
export interface TagsResponse { tags: Tag[]; }

/** A folder in use by at least one link, with how many links carry it. */
export interface Folder { name: string; count: number; }
export interface FoldersResponse { folders: Folder[]; }
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
/** One workspace the current user belongs to (cloud only). */
export interface Membership { tenant_id: number; name: string; slug: string; role: string; }
/**
 * Response of `GET /admin/me`: current principal + whether OIDC is configured.
 * `memberships`/`current_tenant` are present only in cloud mode; their absence
 * means OSS (single-tenant), where the onboarding gate and switcher never show.
 * `current_tenant` is null when the session has no workspace selected yet.
 */
export interface MeResponse {
  authenticated: boolean;
  oidc_enabled: boolean;
  /** Optional custom label for the shared OIDC login button (from
   * `QUARK_OIDC_BUTTON_LABEL`, e.g. "Sign in with Google"). Null/absent when
   * unset, in which case the panel uses its own i18n label. */
  oidc_button_label?: string | null;
  /** True when the server runs in cloud (multi-tenant) mode. Present pre-auth,
   * so the login screen can gate cloud-only affordances like email-first SSO
   * discovery (meaningless in single-tenant OSS). */
  multi_tenant?: boolean;
  /** True when a break-glass admin token is configured (`QUARK_ADMIN_TOKEN`).
   * Present pre-auth so the login screen hides the admin-token field on an
   * SSO-only deployment where it could never work. Absent = assume enabled
   * (backward compatible with older servers). */
  admin_login_enabled?: boolean;
  /** True when the server provisions users through an external IdP (Keycloak,
   * `st.keycloak` set). In this mode invited users are onboarded by an emailed
   * set-password link, so the Members screen shows an "email sent" confirmation
   * instead of a copyable `/invite/<token>` link (that link never onboards a
   * new user under IdP provisioning). Absent/false = the OSS invite-token flow,
   * where the copyable link is the onboarding path. */
  sso_provisioning?: boolean;
  display?: string;
  scopes?: string[];
  memberships?: Membership[];
  current_tenant?: number | null;
  /** `<slug>.<suffix>` domain wildcard for the cloud tenant's own short links; null/absent in OSS or when unconfigured. */
  tenant_domain_suffix?: string | null;
  /** Shared short-link host (`QUARK_PUBLIC_HOST`, e.g. `go.quarkus.com.br`); the
   * fallback host for a tenant without its own subdomain. Null/absent when unset. */
  public_host?: string | null;
  /** Resolved host for building/copying this tenant's short links: primary
   * custom domain → subdomain → shared host. Present once a workspace is
   * selected (cloud); absent in OSS. */
  primary_link_host?: string | null;
}
/** A pending or accepted team invite (cloud only), for the Members screen. */
export interface InviteView {
  id: number;
  email: string;
  role: string;
  expires: number;
  created: number;
}
/** Response of `POST /admin/invites`: the invite record plus the raw token, shown once (the copyable invite link). */
export interface CreateInviteResponse {
  id: number;
  token: string;
  email: string;
  role: string;
  expires: number;
}

export interface PatchLinkRequest { url?: string; ttl?: number | null; tags?: string[]; max_visits?: number | null; rules?: Rule[]; variants?: Variant[]; app_ios?: string | null; app_android?: string | null; folder?: string | null; fallback_url?: string | null; password?: string | null; }

/** The bulk operation applied to a batch of links via `POST /admin/links/bulk`. */
export type BulkOp = "delete" | "add_tag" | "remove_tag" | "set_folder";

/** Per-item outcome in a bulk response. */
export interface BulkItemResult { code: string; ok: boolean; error?: string; }

/** Partial report from `POST /admin/links/bulk`: successes, failures, per-item detail. */
export interface BulkResponse { ok: number; failed: number; results: BulkItemResult[]; }

/** The link events a webhook subscription can be notified about. */
export const WEBHOOK_EVENTS = [
  "link.created",
  "link.updated",
  "link.deleted",
  "link.expired",
  "link.clicked",
  "link.threshold_reached",
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
export type Scope = "links_read" | "links_write" | "webhooks" | "analytics" | "full";
export const ALL_SCOPES: Scope[] = ["links_read", "links_write", "webhooks", "analytics", "full"];

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

/** The two well-known app-association document names the backend accepts. */
export type WellknownName = "apple-app-site-association" | "assetlinks.json";

/**
 * Status of the Google Sheets connector (`GET /admin/integrations/sheets/status`).
 * The refresh token is never part of this shape — it stays server-side.
 * `unavailable` is a panel-only marker (never sent by the server): the status
 * endpoint returns the admin not-found status (401/404) when the connector is
 * off, and `api.sheetsStatus` maps that to `{ connected: false, unavailable: true }`
 * so the Extensions card can render a neutral fallback instead of erroring.
 */
export interface SheetsStatus {
  connected: boolean;
  email?: string;
  spreadsheet_url?: string;
  last_sync?: number;
  last_status: { state: "never" | "ok" | "error"; detail?: string };
  unavailable?: boolean;
}

/** Verification state of a custom domain or SSO email domain. */
export type DomainStatus = "pending" | "verified";

/**
 * An SSO email domain plus the DNS instructions needed to verify it
 * (`GET`/`POST /admin/sso-domains`, `POST /admin/sso-domains/:id/verify`).
 * `txt_name`/`txt_value` are the record a workspace admin must publish at
 * their DNS provider; they stay the same across `verify` calls until the
 * domain is removed and re-added.
 */
export interface SsoDomainView {
  id: number;
  domain: string;
  status: DomainStatus;
  created: number;
  verified_at: number | null;
  txt_name: string;
  txt_value: string;
}

/** A custom link domain (or the tenant's auto subdomain) with its DNS
 * verification instructions. Verified domains serve the tenant's short links. */
export interface LinkDomainView {
  id: number;
  host: string;
  status: DomainStatus;
  created: number;
  verified_at: number | null;
  txt_name: string;
  txt_value: string;
  /** CNAME target to point `host` at; null when the deploy has no shared public host. */
  cname_target: string | null;
  /** True when this is the tenant's primary link domain (copy button + new links use it). */
  primary: boolean;
}

/** The tenant's own OIDC provider, redacted: the `client_secret` never leaves
 * the server, so only `client_secret_set` reports whether one is on file. */
export interface OidcConfigView {
  issuer: string;
  client_id: string;
  scopes: string[];
  admin_claim: string;
  admin_value: string;
  readonly_value: string;
  member_value: string;
  required_value: string | null;
  post_login_url: string | null;
  client_secret_set: boolean;
}

/** Payload to upsert the tenant's OIDC provider. An empty `client_secret`
 * preserves the one already stored (the panel never receives it to echo back). */
export interface PutOidcConfigInput {
  issuer: string;
  client_id: string;
  client_secret: string;
  scopes: string[];
  admin_claim: string;
  admin_value: string;
  readonly_value: string;
  member_value: string;
  required_value: string | null;
  post_login_url: string | null;
}
