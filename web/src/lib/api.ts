import { getToken } from "./auth";
import type {
  ListLinksResponse, CreateLinkRequest, CreateLinkResponse,
  Stats, PatchLinkRequest,
  ListWebhooksResponse, CreateWebhookRequest, CreateWebhookResponse,
  PatchWebhookRequest, TestWebhookResponse,
  ImportSummary, TagsResponse, FoldersResponse,
  ListTokensResponse, CreateTokenRequest, CreateTokenResponse,
  ListPixelsResponse, CreatePixelRequest, Pixel,
  WellknownName, MeResponse, SheetsStatus,
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

/** Absolute URL to start the OIDC login (a full navigation, not fetch). */
export function oidcLoginUrl(): string { return `${BASE}/admin/login`; }

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
   * forced-logout: the header forces a preflight and can't ride a simple POST). */
  async logout(): Promise<void> {
    await req("/admin/logout", { method: "POST" });
  },
  async createLink(body: CreateLinkRequest): Promise<CreateLinkResponse> {
    return jsonOrThrow(await req("/", { method: "POST", body: JSON.stringify(body) }));
  },
  async listLinks(params: { after?: number; limit?: number; q?: string; tag?: string; folder?: string; health?: string } = {}): Promise<ListLinksResponse> {
    const sp = new URLSearchParams();
    if (params.after != null) sp.set("after", String(params.after));
    if (params.limit != null) sp.set("limit", String(params.limit));
    if (params.q && params.q.trim() !== "") sp.set("q", params.q.trim());
    if (params.tag && params.tag.trim() !== "") sp.set("tag", params.tag.trim());
    if (params.folder && params.folder.trim() !== "") sp.set("folder", params.folder.trim());
    if (params.health && params.health.trim() !== "") sp.set("health", params.health.trim());
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
};
