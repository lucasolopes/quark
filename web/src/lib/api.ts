import { getToken } from "./auth";
import type {
  ListLinksResponse, CreateLinkRequest, CreateLinkResponse,
  Stats, BlocklistResponse, PatchLinkRequest,
  ListWebhooksResponse, CreateWebhookRequest, CreateWebhookResponse,
  PatchWebhookRequest, TestWebhookResponse,
  ImportSummary, TagsResponse,
  ListTokensResponse, CreateTokenRequest, CreateTokenResponse,
  ListPixelsResponse, CreatePixelRequest, Pixel,
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
  const res = await fetch(BASE + path, { ...opts, headers });
  if (res.status === 401) { onUnauthorized(); throw new ApiError(401, "unauthorized"); }
  return res;
}

async function jsonOrThrow<T>(res: Response): Promise<T> {
  if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
  return (await res.json()) as T;
}

export const api = {
  async createLink(body: CreateLinkRequest): Promise<CreateLinkResponse> {
    return jsonOrThrow(await req("/", { method: "POST", body: JSON.stringify(body) }));
  },
  async listLinks(params: { after?: number; limit?: number; q?: string; tag?: string } = {}): Promise<ListLinksResponse> {
    const sp = new URLSearchParams();
    if (params.after != null) sp.set("after", String(params.after));
    if (params.limit != null) sp.set("limit", String(params.limit));
    if (params.q && params.q.trim() !== "") sp.set("q", params.q.trim());
    if (params.tag && params.tag.trim() !== "") sp.set("tag", params.tag.trim());
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
  async listBlocked(): Promise<BlocklistResponse> {
    return jsonOrThrow(await req("/admin/blocklist"));
  },
  async listTags(): Promise<TagsResponse> {
    return jsonOrThrow(await req("/admin/tags"));
  },
  async addBlocked(domain: string): Promise<void> {
    const res = await req("/admin/blocklist", { method: "POST", body: JSON.stringify({ domain }) });
    if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
  },
  async removeBlocked(domain: string): Promise<void> {
    const res = await req("/admin/blocklist", { method: "DELETE", body: JSON.stringify({ domain }) });
    if (!res.ok) throw new ApiError(res.status, await res.text().catch(() => res.statusText));
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
};
