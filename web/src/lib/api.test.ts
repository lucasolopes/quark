import { describe, it, expect, beforeEach, vi } from "vitest";
import { api, ApiError, oidcLoginUrl, setUnauthorizedHandler } from "./api";
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

describe("invite endpoints", () => {
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

  it("createInvite posts email+role to /admin/invites and returns the token", async () => {
    const spy = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(
        JSON.stringify({ id: 1, token: "inv_abc", email: "a@b.com", role: "member", expires: 123 }),
        { status: 200 },
      ),
    );
    const r = await api.createInvite("a@b.com", "member");
    expect(r.token).toBe("inv_abc");
    const [url, init] = spy.mock.calls[0];
    expect(String(url)).toContain("/admin/invites");
    expect(init?.method).toBe("POST");
    expect(JSON.parse(String(init?.body))).toEqual({ email: "a@b.com", role: "member" });
  });

  it("acceptInvite posts to the token accept path and returns the membership", async () => {
    const spy = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ tenant_id: 9, role: "member" }), { status: 200 }),
    );
    const r = await api.acceptInvite("inv_abc");
    expect(r).toEqual({ tenant_id: 9, role: "member" });
    const [url, init] = spy.mock.calls[0];
    expect(String(url)).toContain("/admin/invites/inv_abc/accept");
    expect(init?.method).toBe("POST");
  });

  it("acceptInvite throws ApiError(409) when the user is already a member", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 409 }));
    await expect(api.acceptInvite("inv_abc")).rejects.toMatchObject({ status: 409 });
  });

  it("acceptInvite returns a model-B login_required body verbatim", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ status: "login_required", login_url: "/admin/login?org=acme" }), { status: 200 }),
    );
    const r = await api.acceptInvite("inv_abc");
    expect(r).toEqual({ status: "login_required", login_url: "/admin/login?org=acme" });
  });

  it("acceptInvite returns the model-A body unchanged (no status field)", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ tenant_id: 9, role: "member" }), { status: 200 }),
    );
    const r = await api.acceptInvite("inv_abc");
    expect(r).toEqual({ tenant_id: 9, role: "member" });
    expect((r as { status?: string }).status).toBeUndefined();
  });
});

describe("api.discoverSso", () => {
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

  it("returns the org when the email's domain is a verified SSO domain", async () => {
    const spy = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ org: "acme" }), { status: 200 }),
    );
    const r = await api.discoverSso("jane@acme.com");
    expect(r).toEqual({ org: "acme" });
    const [url] = spy.mock.calls[0];
    expect(String(url)).toContain("/admin/sso/discover?email=");
    expect(String(url)).toContain(encodeURIComponent("jane@acme.com"));
  });

  it("returns an empty object when the domain has no SSO org", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({}), { status: 200 }));
    const r = await api.discoverSso("jane@personal.com");
    expect(r).toEqual({});
  });

  it("encodes special characters in the email", async () => {
    const spy = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({}), { status: 200 }));
    await api.discoverSso("jane+test@acme.com");
    const [url] = spy.mock.calls[0];
    expect(String(url)).toContain(`email=${encodeURIComponent("jane+test@acme.com")}`);
  });
});

describe("oidcLoginUrl", () => {
  it("with no org, points at /admin/login with no ?org", () => {
    const url = oidcLoginUrl();
    expect(url.endsWith("/admin/login")).toBe(true);
    expect(url).not.toContain("?org");
  });

  it("with an org, appends ?org=<encoded org>", () => {
    const url = oidcLoginUrl("acme");
    expect(url).toContain("/admin/login?org=acme");
  });

  it("encodes an org slug with special characters", () => {
    const url = oidcLoginUrl("a b");
    expect(url).toContain(`/admin/login?org=${encodeURIComponent("a b")}`);
  });
});

describe("SSO email domain endpoints", () => {
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

  it("oidcConfigured returns true on 200", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({}), { status: 200 }));
    expect(await api.oidcConfigured()).toBe(true);
  });

  it("oidcConfigured returns false on 404 (no oidc provider set up yet)", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 404 }));
    expect(await api.oidcConfigured()).toBe(false);
  });

  it("listSsoDomains GETs /admin/sso-domains", async () => {
    const spy = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify([]), { status: 200 }));
    await api.listSsoDomains();
    const [url, init] = spy.mock.calls[0];
    expect(String(url)).toContain("/admin/sso-domains");
    expect(init?.method ?? "GET").toBe("GET");
  });

  it("createSsoDomain posts {domain} to /admin/sso-domains", async () => {
    const spy = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(
        JSON.stringify({
          id: 1, domain: "acme.com", status: "pending", created: 1, verified_at: null,
          txt_name: "_quark-sso.acme.com", txt_value: "tok_abc",
        }),
        { status: 200 },
      ),
    );
    const d = await api.createSsoDomain("acme.com");
    expect(d.domain).toBe("acme.com");
    const [url, init] = spy.mock.calls[0];
    expect(String(url)).toContain("/admin/sso-domains");
    expect(init?.method).toBe("POST");
    expect(JSON.parse(String(init?.body))).toEqual({ domain: "acme.com" });
  });

  it("verifySsoDomain posts to /admin/sso-domains/:id/verify", async () => {
    const spy = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(
        JSON.stringify({
          id: 1, domain: "acme.com", status: "verified", created: 1, verified_at: 2,
          txt_name: "_quark-sso.acme.com", txt_value: "tok_abc",
        }),
        { status: 200 },
      ),
    );
    const d = await api.verifySsoDomain(1);
    expect(d.status).toBe("verified");
    const [url, init] = spy.mock.calls[0];
    expect(String(url)).toContain("/admin/sso-domains/1/verify");
    expect(init?.method).toBe("POST");
  });

  it("deleteSsoDomain DELETEs /admin/sso-domains/:id", async () => {
    const spy = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(null, { status: 204 }));
    await api.deleteSsoDomain(7);
    const [url, init] = spy.mock.calls[0];
    expect(String(url)).toContain("/admin/sso-domains/7");
    expect(init?.method).toBe("DELETE");
  });

  it("deleteSsoDomain throws ApiError on a non-ok response", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("not found", { status: 404 }));
    await expect(api.deleteSsoDomain(7)).rejects.toMatchObject({ status: 404 });
  });
});
