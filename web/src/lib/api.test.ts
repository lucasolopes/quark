import { describe, it, expect, beforeEach, vi } from "vitest";
import { api, ApiError, setUnauthorizedHandler } from "./api";
import { setToken } from "./auth";

describe("api client", () => {
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

  it("sends x-admin-token and parses JSON", async () => {
    setToken("secret");
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ links: [], next_after: null }), { status: 200 }),
    );
    const r = await api.listLinks({ limit: 10 });
    expect(r.links).toEqual([]);
    const [, init] = fetchMock.mock.calls[0];
    expect(new Headers(init!.headers).get("x-admin-token")).toBe("secret");
  });

  it("401 triggers onUnauthorized and throws ApiError", async () => {
    const onUnauth = vi.fn();
    setUnauthorizedHandler(onUnauth);
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 401 }));
    await expect(api.listLinks()).rejects.toBeInstanceOf(ApiError);
    expect(onUnauth).toHaveBeenCalledOnce();
  });

  it("a non-ok response becomes an ApiError with status", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("blocked destination", { status: 403 }));
    await expect(api.createLink({ url: "https://x.com" })).rejects.toMatchObject({ status: 403 });
  });

  it("listLinks includes q in the querystring when provided (and omits it when empty)", async () => {
    const fetchMock = vi
      .spyOn(globalThis, "fetch")
      .mockImplementation(() => Promise.resolve(new Response(JSON.stringify({ links: [], next_after: null }), { status: 200 })));
    await api.listLinks({ q: "git", limit: 50 });
    const [url] = fetchMock.mock.calls[0];
    expect(String(url)).toContain("q=git");

    await api.listLinks({ q: "  " });
    const [url2] = fetchMock.mock.calls[1];
    expect(String(url2)).not.toContain("q=");
  });
});
