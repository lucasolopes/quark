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

  it("listLinks includes tag in the querystring when provided", async () => {
    const fetchMock = vi
      .spyOn(globalThis, "fetch")
      .mockImplementation(() => Promise.resolve(new Response(JSON.stringify({ links: [], next_after: null }), { status: 200 })));
    await api.listLinks({ tag: "promo" });
    const [url] = fetchMock.mock.calls[0];
    expect(String(url)).toContain("tag=promo");
  });

  it("listTags fetches /admin/tags", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(
        JSON.stringify({ tags: [{ name: "promo", count: 3 }, { name: "summer", count: 1 }] }),
        { status: 200 },
      ),
    );
    const r = await api.listTags();
    expect(r.tags).toEqual([{ name: "promo", count: 3 }, { name: "summer", count: 1 }]);
  });
});

describe("workspace endpoints", () => {
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

  it("createWorkspace posts name+slug to /admin/tenants and returns the tenant", async () => {
    const spy = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ id: 5, name: "Acme", slug: "acme", created: 1 }), { status: 200 }),
    );
    const t = await api.createWorkspace("Acme", "acme");
    expect(t.id).toBe(5);
    const [url, init] = spy.mock.calls[0];
    expect(String(url)).toContain("/admin/tenants");
    expect(init?.method).toBe("POST");
    expect(JSON.parse(String(init?.body))).toEqual({ name: "Acme", slug: "acme" });
  });

  it("createWorkspace throws ApiError(409) on a duplicate slug", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 409 }));
    await expect(api.createWorkspace("Acme", "acme")).rejects.toMatchObject({ status: 409 });
  });

  it("switchWorkspace posts tenant_id to /admin/workspace/switch", async () => {
    const spy = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 200 }));
    await api.switchWorkspace(7);
    const [url, init] = spy.mock.calls[0];
    expect(String(url)).toContain("/admin/workspace/switch");
    expect(JSON.parse(String(init?.body))).toEqual({ tenant_id: 7 });
  });

  it("switchWorkspace throws ApiError(403) when the user lacks membership", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 403 }));
    await expect(api.switchWorkspace(7)).rejects.toMatchObject({ status: 403 });
  });
});
