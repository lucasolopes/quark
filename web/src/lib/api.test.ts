import { describe, it, expect, beforeEach, vi } from "vitest";
import { api, ApiError, setUnauthorizedHandler } from "./api";
import { setToken } from "./auth";

describe("api client", () => {
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

  it("envia x-admin-token e parseia JSON", async () => {
    setToken("segredo");
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ links: [], next_after: null }), { status: 200 }),
    );
    const r = await api.listLinks({ limit: 10 });
    expect(r.links).toEqual([]);
    const [, init] = fetchMock.mock.calls[0];
    expect(new Headers(init!.headers).get("x-admin-token")).toBe("segredo");
  });

  it("401 dispara onUnauthorized e lança ApiError", async () => {
    const onUnauth = vi.fn();
    setUnauthorizedHandler(onUnauth);
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 401 }));
    await expect(api.listLinks()).rejects.toBeInstanceOf(ApiError);
    expect(onUnauth).toHaveBeenCalledOnce();
  });

  it("erro !ok vira ApiError com status", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("destino bloqueado", { status: 403 }));
    await expect(api.createLink({ url: "https://x.com" })).rejects.toMatchObject({ status: 403 });
  });

  it("listLinks inclui q no querystring quando informado (e omite quando vazio)", async () => {
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
