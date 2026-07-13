import { getToken } from "./auth";
import type {
  ListLinksResponse, CreateLinkRequest, CreateLinkResponse,
  Stats, BlocklistResponse, PatchLinkRequest,
} from "./types";

const BASE: string = (import.meta.env.VITE_API_BASE_URL as string | undefined) ?? "";

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
  if (res.status === 401) { onUnauthorized(); throw new ApiError(401, "não autorizado"); }
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
  async listLinks(params: { after?: number; limit?: number } = {}): Promise<ListLinksResponse> {
    const q = new URLSearchParams();
    if (params.after != null) q.set("after", String(params.after));
    if (params.limit != null) q.set("limit", String(params.limit));
    const qs = q.toString();
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
};
