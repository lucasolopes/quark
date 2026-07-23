import { getToken } from "./auth";
import type {
  ListLinksResponse, CreateLinkRequest, CreateLinkResponse,
  Stats, PatchLinkRequest,
  ListWebhooksResponse, CreateWebhookRequest, CreateWebhookResponse,
  PatchWebhookRequest, TestWebhookResponse,
  ImportSummary, TagsResponse, FoldersResponse,
  BulkOp, BulkResponse,
  ListTokensResponse, CreateTokenRequest, CreateTokenResponse,
  ListPixelsResponse, CreatePixelRequest, Pixel,
  WellknownName, MeResponse, SheetsStatus,
  InviteView, CreateInviteResponse,
  SsoDomainView, AlertRule,
  OidcConfigView, PutOidcConfigInput, LinkDomainView,
} from "./types";

/**
 * Strips trailing slash(es) from the env var — avoids `//` when concatenated
 * with the path (which already starts with `/`), in case the env has a
 * trailing slash.
 */
const BASE: string = ((import.meta.env.VITE_API_BASE_URL as string | undefined) ?? "").replace(/\/+$/, "");

let onUnauthorized: () => void = () => {};
export function setUnauthorizedHandler(fn: () => void): void { onUnauthorized = fn; }

export class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
    this.name = "ApiError";
  }
}

async function req(path: string, opts: RequestInit = {}): Promise<Response> {
  const headers = new Headers(opts.headers);
  const token = getToken();
  if (token) headers.set("x-admin-token", token);
  if (opts.body && !headers.has("content-type")) headers.set("content-type", "application/json");
  // Custom header on every state-changing request: the server requires it for
  // cookie-authenticated simple POSTs (defeats cross-site CSRF), and it forces a
  // CORS preflight so a cross-origin panel is gated by the allowlist.
  const method = (opts.method ?? "GET").toUpperCase();
  if (method !== "GET" && method !== "HEAD") headers.set("x-quark-csrf", "1");
  // `include` so the OIDC session cookie is sent (also on a cross-origin panel).
  const res = await fetch(BASE + path, { ...opts, headers, credentials: "include" });
  if (res.status === 401) { onUnauthorized(); throw new ApiError(401, "unauthorized"); }
  return res;
}

/**
 * Absolute URL to start the OIDC login (a full navigation, not fetch). An
 * optional `org` is a per-tenant SSO hint and `email` becomes the IdP
 * `login_hint` so the provider pre-fills the username (both untrusted UX
 * conveniences — the server decides what to do with them, we just forward).
 */
export function oidcLoginUrl(org?: string, email?: string): string {
  const parts: string[] = [];
  if (org) parts.push(`org=${encodeURIComponent(org)}`);
  const hint = email?.trim();
  if (hint) parts.push(`login_hint=${encodeURIComponent(hint)}`);
  return parts.length ? `${BASE}/admin/login?${parts.join("&")}` : `${BASE}/admin/login`;
}

async function jsonOrThrow<T>(res: Response): Promise<T> {
  if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
  return (await res.json()) as T;
}

export const api = {
  /** Current principal + whether OIDC is configured (for the login screen). */
  async me(): Promise<MeResponse> {
    return jsonOrThrow(await req("/admin/me"));
  },
  /** Revokes the current OIDC session server-side and clears its cookie. `req`
   * attaches the `x-quark-csrf` header the server requires (defeats cross-site
   * forced-logout: the header forces a preflight and can't ride a simple POST).
   * Returns `{ logout_url }`: when non-null, the caller should navigate the
   * browser there to end the IdP session too (RP-initiated logout, LUC-79). */
  async logout(): Promise<{ logout_url: string | null }> {
    return jsonOrThrow(await req("/admin/logout", { method: "POST" }));
  },
  /** Creates a workspace (cloud only) and re-points the session at it. 409 if the slug is taken, 429 if rate-limited. */
  async createWorkspace(name: string, slug: string): Promise<{ id: number; name: string; slug: string; created: number }> {
    return jsonOrThrow(await req("/admin/tenants", { method: "POST", body: JSON.stringify({ name, slug }) }));
  },
  /** Switches the session's current workspace (cloud only). 403 if the user has no membership in `tenantId`. */
  async switchWorkspace(tenantId: number): Promise<void> {
    const res = await req("/admin/workspace/switch", { method: "POST", body: JSON.stringify({ tenant_id: tenantId }) });
    if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
  },
  async createLink(body: CreateLinkRequest): Promise<CreateLinkResponse> {
    return jsonOrThrow(await req("/", { method: "POST", body: JSON.stringify(body) }));
  },
  async listLinks(params: { after?: number; limit?: number; q?: string; tag?: string; folder?: string; health?: string; status?: string } = {}): Promise<ListLinksResponse> {
    const sp = new URLSearchParams();
    if (params.after != null) sp.set("after", String(params.after));
    if (params.limit != null) sp.set("limit", String(params.limit));
    if (params.q && params.q.trim() !== "") sp.set("q", params.q.trim());
    if (params.tag && params.tag.trim() !== "") sp.set("tag", params.tag.trim());
    if (params.folder && params.folder.trim() !== "") sp.set("folder", params.folder.trim());
    if (params.health && params.health.trim() !== "") sp.set("health", params.health.trim());
    if (params.status && params.status.trim() !== "") sp.set("status", params.status.trim());
    const qs = sp.toString();
    return jsonOrThrow(await req(`/admin/links${qs ? `?${qs}` : ""}`));
  },
  async deleteLink(code: string): Promise<void> {
    const res = await req(`/admin/links/${encodeURIComponent(code)}`, { method: "DELETE" });
    if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
  },
  async patchLink(code: string, body: PatchLinkRequest): Promise<void> {
    const res = await req(`/admin/links/${encodeURIComponent(code)}`, {
      method: "PATCH", body: JSON.stringify(body),
    });
    if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
  },
  /**
   * Applies one operation (`delete`/`add_tag`/`remove_tag`/`set_folder`) to a
   * batch of links. `value` is the tag or folder (omit for `delete`; empty on
   * `set_folder` clears the folder). Returns a partial report — a per-item
   * failure does not fail the request.
   */
  async bulkLinks(codes: string[], op: BulkOp, value?: string): Promise<BulkResponse> {
    return jsonOrThrow(
      await req("/admin/links/bulk", { method: "POST", body: JSON.stringify({ codes, op, value }) }),
    );
  },
  /** The link's current click-threshold alert rule, or `null` when unset (LUC-66). */
  async getLinkAlert(code: string): Promise<AlertRule | null> {
    return jsonOrThrow(await req(`/admin/links/${encodeURIComponent(code)}/alert`));
  },
  /** Sets (or replaces) the link's click-threshold alert rule. `window_secs` must be >= 60. */
  async setLinkAlert(code: string, body: AlertRule): Promise<AlertRule> {
    return jsonOrThrow(
      await req(`/admin/links/${encodeURIComponent(code)}/alert`, { method: "PUT", body: JSON.stringify(body) }),
    );
  },
  /** Removes the link's alert rule; a missing rule is not an error. */
  async deleteLinkAlert(code: string): Promise<void> {
    const res = await req(`/admin/links/${encodeURIComponent(code)}/alert`, { method: "DELETE" });
    if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
  },
  async getStats(code: string): Promise<Stats> {
    return jsonOrThrow(await req(`/${encodeURIComponent(code)}/stats`));
  },
  async listTags(): Promise<TagsResponse> {
    return jsonOrThrow(await req("/admin/tags"));
  },
  async listFolders(): Promise<FoldersResponse> {
    return jsonOrThrow(await req("/admin/folders"));
  },
  async listWebhooks(): Promise<ListWebhooksResponse> {
    return jsonOrThrow(await req("/admin/webhooks"));
  },
  async createWebhook(body: CreateWebhookRequest): Promise<CreateWebhookResponse> {
    return jsonOrThrow(await req("/admin/webhooks", { method: "POST", body: JSON.stringify(body) }));
  },
  async patchWebhook(id: number, body: PatchWebhookRequest): Promise<void> {
    const res = await req(`/admin/webhooks/${id}`, { method: "PATCH", body: JSON.stringify(body) });
    if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
  },
  async deleteWebhook(id: number): Promise<void> {
    const res = await req(`/admin/webhooks/${id}`, { method: "DELETE" });
    if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
  },
  async testWebhook(id: number): Promise<TestWebhookResponse> {
    return jsonOrThrow(await req(`/admin/webhooks/${id}/test`, { method: "POST" }));
  },
  /**
   * Bulk-imports links from a raw CSV or JSON body. `contentType` picks the
   * parser server-side (`text/csv` or `application/json`) and is sent as-is,
   * overriding the default `application/json` the shared `req` helper would
   * otherwise set for any request with a body.
   */
  async importLinks(body: string, contentType: string): Promise<ImportSummary> {
    return jsonOrThrow(
      await req("/admin/import", { method: "POST", body, headers: { "content-type": contentType } }),
    );
  },
  async listTokens(): Promise<ListTokensResponse> {
    return jsonOrThrow(await req("/admin/tokens"));
  },
  async createToken(body: CreateTokenRequest): Promise<CreateTokenResponse> {
    return jsonOrThrow(await req("/admin/tokens", { method: "POST", body: JSON.stringify(body) }));
  },
  async deleteToken(id: number): Promise<void> {
    const res = await req(`/admin/tokens/${id}`, { method: "DELETE" });
    if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
  },
  async listPixels(): Promise<ListPixelsResponse> {
    return jsonOrThrow(await req("/admin/pixels"));
  },
  async createPixel(body: CreatePixelRequest): Promise<Pixel> {
    return jsonOrThrow(await req("/admin/pixels", { method: "POST", body: JSON.stringify(body) }));
  },
  async deletePixel(id: number): Promise<void> {
    const res = await req(`/admin/pixels/${encodeURIComponent(String(id))}`, { method: "DELETE" });
    if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
  },
  async getWellknown(name: WellknownName): Promise<string | null> {
    const res = await req(`/admin/wellknown/${encodeURIComponent(name)}`);
    if (res.status === 404) return null;
    if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
    const body = await res.text();
    return body === "" ? null : body;
  },
  async putWellknown(name: WellknownName, body: string): Promise<void> {
    const res = await req(`/admin/wellknown/${encodeURIComponent(name)}`, { method: "PUT", body });
    if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
  },
  async deleteWellknown(name: WellknownName): Promise<void> {
    const res = await req(`/admin/wellknown/${encodeURIComponent(name)}`, { method: "DELETE" });
    if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
  },
  /**
   * Google Sheets connector status. The connector-off case returns the admin
   * not-found status (401 when an admin credential exists, else 404) — the same
   * as a genuine auth failure. We must NOT route it through `req`, whose 401
   * path calls the global `onUnauthorized` handler: an operator viewing the
   * Extensions page with the connector off would be bounced to /login. So this
   * does a raw fetch and maps any non-OK to a neutral "unavailable" status the
   * card renders as the old "via Webhooks" fallback (it never throws to the
   * error boundary).
   */
  async sheetsStatus(): Promise<SheetsStatus> {
    const headers = new Headers();
    const token = getToken();
    if (token) headers.set("x-admin-token", token);
    const res = await fetch(BASE + "/admin/integrations/sheets/status", { headers, credentials: "include" });
    if (!res.ok) return { connected: false, unavailable: true, last_status: { state: "never" } };
    return (await res.json()) as SheetsStatus;
  },
  /** Starts the Google OAuth connect: returns the consent URL to navigate to (the server also sets a signed state cookie). */
  async sheetsConnect(): Promise<{ url: string }> {
    return jsonOrThrow(await req("/admin/integrations/sheets/connect"));
  },
  /** Runs one on-demand sync. Returns the same shape as `sheetsStatus`; a sync error comes back as 200 with `last_status.state === "error"`. */
  async sheetsSync(): Promise<SheetsStatus> {
    return jsonOrThrow(await req("/admin/integrations/sheets/sync", { method: "POST" }));
  },
  /** Disconnects the connector (drops the stored connection, including the refresh token). */
  async sheetsDisconnect(): Promise<void> {
    const res = await req("/admin/integrations/sheets", { method: "DELETE" });
    if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
  },
  /** Starts the Slack "Add to Slack" OAuth install: returns the authorize URL to navigate to (the server also sets a signed state cookie). */
  async slackConnect(): Promise<{ url: string }> {
    return jsonOrThrow(await req("/admin/integrations/slack/connect"));
  },
  /** Pending and accepted team invites for the current workspace (cloud only). */
  async listInvites(): Promise<InviteView[]> {
    return jsonOrThrow(await req("/admin/invites"));
  },
  /** Invites an email to the current workspace with the given role. Returns the invite plus its raw token (shown once, the copyable invite link). */
  async createInvite(email: string, role: string): Promise<CreateInviteResponse> {
    return jsonOrThrow(await req("/admin/invites", { method: "POST", body: JSON.stringify({ email, role }) }));
  },
  /** Revokes a pending invite. */
  async revokeInvite(id: number): Promise<void> {
    const res = await req(`/admin/invites/${id}`, { method: "DELETE" });
    if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
  },
  /**
   * Accepts an invite by token, granting the current user membership in its
   * workspace. 409 if already a member, 410/404 if expired or unknown. The
   * body is model-A membership (`{ tenant_id, role }`) or, for a tenant on
   * model B, `{ status: "login_required", login_url }` — the caller decides
   * what to do with either shape.
   */
  async acceptInvite(token: string): Promise<{ status?: string; login_url?: string }> {
    return jsonOrThrow(await req(`/admin/invites/${encodeURIComponent(token)}/accept`, { method: "POST" }));
  },
  /**
   * Looks up the SSO org for an email's domain, for the email-first login
   * step. Uniform 200 either way — `{org}` when the domain is a verified SSO
   * domain of an oidc-configured tenant, else `{}`. The email is an untrusted
   * UX hint we just forward; the server owns whether it means anything.
   */
  async discoverSso(email: string): Promise<{ org?: string }> {
    return jsonOrThrow(await req(`/admin/sso/discover?email=${encodeURIComponent(email)}`));
  },
  /**
   * Whether the current workspace has its own OIDC provider configured — the
   * prerequisite for SSO email domains to route anywhere. A plain fetch, not
   * `req`: an unconfigured workspace is a normal 404 here, not an auth
   * failure, so this must not trigger the global `onUnauthorized` redirect
   * (mirrors `sheetsStatus`).
   */
  async oidcConfigured(): Promise<boolean> {
    const headers = new Headers();
    const token = getToken();
    if (token) headers.set("x-admin-token", token);
    const res = await fetch(BASE + "/admin/oidc-config", { headers, credentials: "include" });
    return res.ok;
  },
  /** Lists the current workspace's link domains (custom + the auto subdomain), each with DNS instructions (cloud only). */
  async listDomains(): Promise<LinkDomainView[]> {
    return jsonOrThrow(await req("/admin/domains"));
  },
  /** Registers a custom link domain (pending DNS verification). 409 if taken; 400 on an implausible host. */
  async createDomain(host: string): Promise<LinkDomainView> {
    return jsonOrThrow(await req("/admin/domains", { method: "POST", body: JSON.stringify({ host }) }));
  },
  /** Checks the domain's `_quark-verify.<host>` TXT record and flips it to verified on a match; returns the domain either way. */
  async verifyDomain(id: number): Promise<LinkDomainView> {
    return jsonOrThrow(await req(`/admin/domains/${id}/verify`, { method: "POST" }));
  },
  /** Removes a custom link domain. */
  async deleteDomain(id: number): Promise<void> {
    const res = await req(`/admin/domains/${id}`, { method: "DELETE" });
    if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
  },
  /** Makes a verified domain the tenant's primary link domain (copy button + new links use it). */
  async setPrimaryDomain(id: number): Promise<LinkDomainView> {
    return jsonOrThrow(await req(`/admin/domains/${id}/primary`, { method: "POST" }));
  },
  /** The current workspace's own OIDC provider, redacted (never the secret). Throws 404 when none is configured. */
  async getOidcConfig(): Promise<OidcConfigView> {
    return jsonOrThrow(await req("/admin/oidc-config"));
  },
  /** Upserts the workspace's OIDC provider. An empty `client_secret` keeps the stored one. */
  async putOidcConfig(input: PutOidcConfigInput): Promise<OidcConfigView> {
    return jsonOrThrow(await req("/admin/oidc-config", { method: "PUT", body: JSON.stringify(input) }));
  },
  /** Removes the workspace's OIDC provider (falls back to the shared/global login). */
  async deleteOidcConfig(): Promise<void> {
    const res = await req("/admin/oidc-config", { method: "DELETE" });
    if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
  },
  /** Lists the current workspace's SSO email domains with their DNS verification instructions (cloud only). */
  async listSsoDomains(): Promise<SsoDomainView[]> {
    return jsonOrThrow(await req("/admin/sso-domains"));
  },
  /**
   * Registers an email domain for SSO discovery under the current workspace,
   * pending DNS verification. 409 if the domain is taken or the workspace has
   * no OIDC provider configured yet; 400 on an implausible domain.
   */
  async createSsoDomain(domain: string): Promise<SsoDomainView> {
    return jsonOrThrow(await req("/admin/sso-domains", { method: "POST", body: JSON.stringify({ domain }) }));
  },
  /** Checks the domain's `_quark-sso.<domain>` TXT record and flips it to verified on a match; returns the domain either way. */
  async verifySsoDomain(id: number): Promise<SsoDomainView> {
    return jsonOrThrow(await req(`/admin/sso-domains/${id}/verify`, { method: "POST" }));
  },
  /** Removes an SSO email domain from the current workspace. */
  async deleteSsoDomain(id: number): Promise<void> {
    const res = await req(`/admin/sso-domains/${id}`, { method: "DELETE" });
    if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
  },
};
