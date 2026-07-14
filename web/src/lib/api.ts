import { getToken } from "./auth";
import type {
  ListLinksResponse, CreateLinkRequest, CreateLinkResponse,
  Stats, BlocklistResponse, PatchLinkRequest,
  ListWebhooksResponse, CreateWebhookRequest, CreateWebhookResponse,
  PatchWebhookRequest, TestWebhookResponse,
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
  if (opts.body) headers.set("content-type", "application/json");
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
  async listLinks(params: { after?: number; limit?: number; q?: string } = {}): Promise<ListLinksResponse> {
    const sp = new URLSearchParams();
    if (params.after != null) sp.set("after", String(params.after));
    if (params.limit != null) sp.set("limit", String(params.limit));
    if (params.q && params.q.trim() !== "") sp.set("q", params.q.trim());
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
};
