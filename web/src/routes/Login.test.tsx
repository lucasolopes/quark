import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Login } from "./Login";
import { withProviders } from "@/test-utils";

/**
 * Fetch mock that branches on the request URL: `/admin/me` gets `meBody`,
 * `/admin/sso/discover` gets `discoverBody`, anything else falls back to an
 * empty 200 (e.g. the token-submit probe `listLinks`).
 */
function mockFetchByUrl(meBody: unknown, discoverBody: unknown = {}) {
  vi.spyOn(globalThis, "fetch").mockImplementation((input: RequestInfo | URL) => {
    const url = String(input);
    if (url.includes("/admin/me")) {
      return Promise.resolve(new Response(JSON.stringify(meBody), { status: 200 }));
    }
    if (url.includes("/admin/sso/discover")) {
      return Promise.resolve(new Response(JSON.stringify(discoverBody), { status: 200 }));
    }
    return Promise.resolve(new Response(JSON.stringify({ links: [], next_after: null }), { status: 200 }));
  });
}

describe("Login", () => {
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

  it("valid token is stored and the probe request is made", async () => {
    // Fresh Response per call: the Login mount also probes GET /admin/me, and a
    // single Response instance's body can only be read once.
    vi.spyOn(globalThis, "fetch").mockImplementation(() =>
      Promise.resolve(new Response(JSON.stringify({ links: [], next_after: null }), { status: 200 })),
    );
    render(withProviders(<Login />));
    await userEvent.type(screen.getByLabelText(/token/i), "secret");
    await userEvent.click(screen.getByRole("button", { name: /sign in/i }));
    expect(localStorage.getItem("quark_admin_token")).toBe("secret");
  });

  it("hides the provider button and the email step when OIDC is disabled", async () => {
    mockFetchByUrl({ authenticated: false, oidc_enabled: false });
    render(withProviders(<Login />));
    // Give the mount probe time to resolve.
    await screen.findByLabelText(/token/i);
    expect(screen.queryByRole("button", { name: /provider/i })).not.toBeInTheDocument();
    expect(screen.queryByLabelText(/^email$/i)).not.toBeInTheDocument();
  });

  it("hides the admin token field when the server has no admin token (SSO-only)", async () => {
    // admin_login_enabled:false -> the break-glass token field must not render;
    // the SSO path (email step) is the only way in (LUC-75).
    mockFetchByUrl({ authenticated: false, oidc_enabled: true, multi_tenant: true, admin_login_enabled: false });
    render(withProviders(<Login />, { initialEntries: ["/login"] }));
    expect(await screen.findByLabelText(/^email$/i)).toBeInTheDocument();
    expect(screen.queryByLabelText(/token/i)).not.toBeInTheDocument();
  });

  it("keeps the admin token field when admin_login_enabled is true", async () => {
    mockFetchByUrl({ authenticated: false, oidc_enabled: true, multi_tenant: true, admin_login_enabled: true });
    render(withProviders(<Login />, { initialEntries: ["/login"] }));
    expect(await screen.findByLabelText(/token/i)).toBeInTheDocument();
  });

  it("hides the email step on OSS even when a shared OIDC provider is configured", async () => {
    // Single-tenant OSS with a global OIDC provider: home-realm discovery is
    // meaningless, so the email-first step must not appear (it would only fire
    // a discover request that 404s). The shared provider button still shows.
    mockFetchByUrl({ authenticated: false, oidc_enabled: true, multi_tenant: false });
    render(withProviders(<Login />, { initialEntries: ["/login"] }));
    expect(await screen.findByRole("button", { name: /provider/i })).toBeInTheDocument();
    expect(screen.queryByLabelText(/^email$/i)).not.toBeInTheDocument();
    expect(screen.getByLabelText(/token/i)).toBeInTheDocument();
  });

  it("invalid token (401) shows a specific error", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 401 }));
    render(withProviders(<Login />));
    await userEvent.type(screen.getByLabelText(/token/i), "wrong");
    await userEvent.click(screen.getByRole("button", { name: /sign in/i }));
    expect(await screen.findByText(/invalid token/i)).toBeInTheDocument();
  });

  it("network/5xx error shows a generic connection message, not 'invalid token'", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 503 }));
    render(withProviders(<Login />));
    await userEvent.type(screen.getByLabelText(/token/i), "secret");
    await userEvent.click(screen.getByRole("button", { name: /sign in/i }));
    expect(await screen.findByText(/could not connect/i)).toBeInTheDocument();
    expect(screen.queryByText(/invalid token/i)).not.toBeInTheDocument();
  });

  it("with ?org= and OIDC enabled, shows the org header and a per-tenant sign-in button (skips the email step)", async () => {
    mockFetchByUrl({ authenticated: false, oidc_enabled: true, multi_tenant: true });
    render(withProviders(<Login />, { initialEntries: ["/login?org=acme"] }));
    expect(await screen.findByText(/organization: acme/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /sign in to acme/i })).toBeInTheDocument();
    // Token form stays present alongside the org-aware provider button.
    expect(screen.getByLabelText(/token/i)).toBeInTheDocument();
    // The email-first step never shows once ?org= picks the tenant.
    expect(screen.queryByLabelText(/^email$/i)).not.toBeInTheDocument();
  });

  it("clicking the per-tenant button navigates to /admin/login?org=acme", async () => {
    mockFetchByUrl({ authenticated: false, oidc_enabled: true, multi_tenant: true });
    const originalLocation = window.location;
    // @ts-expect-error -- replacing window.location for assertion, restored below.
    delete window.location;
    window.location = { ...originalLocation, href: "" } as unknown as string & Location;
    render(withProviders(<Login />, { initialEntries: ["/login?org=acme"] }));
    await userEvent.click(await screen.findByRole("button", { name: /sign in to acme/i }));
    expect(window.location.href).toContain("/admin/login?org=acme");
    window.location = originalLocation as unknown as string & Location;
  });

  it("with no ?org= and OIDC enabled, shows the email-first step instead of the provider button", async () => {
    mockFetchByUrl({ authenticated: false, oidc_enabled: true, multi_tenant: true });
    render(withProviders(<Login />, { initialEntries: ["/login"] }));
    expect(await screen.findByLabelText(/^email$/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /continue/i })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /provider/i })).not.toBeInTheDocument();
    // Token form is still reachable.
    expect(screen.getByLabelText(/token/i)).toBeInTheDocument();
  });

  it("submitting an email whose domain resolves to an org routes straight to that tenant's SSO", async () => {
    mockFetchByUrl({ authenticated: false, oidc_enabled: true, multi_tenant: true }, { org: "acme" });
    const originalLocation = window.location;
    // @ts-expect-error -- replacing window.location for assertion, restored below.
    delete window.location;
    window.location = { ...originalLocation, href: "" } as unknown as string & Location;
    render(withProviders(<Login />, { initialEntries: ["/login"] }));
    await userEvent.type(await screen.findByLabelText(/^email$/i), "jane@acme.com");
    await userEvent.click(screen.getByRole("button", { name: /continue/i }));
    await vi.waitFor(() => expect(window.location.href).toContain("/admin/login?org=acme"));
    // The typed email is forwarded as login_hint so the IdP pre-fills it (LUC-76).
    expect(window.location.href).toContain("login_hint=jane%40acme.com");
    window.location = originalLocation as unknown as string & Location;
  });

  it("submitting an email with no SSO org falls back to the shared provider button and token field (no redirect)", async () => {
    mockFetchByUrl({ authenticated: false, oidc_enabled: true, multi_tenant: true }, {});
    const originalLocation = window.location;
    // @ts-expect-error -- replacing window.location for assertion, restored below.
    delete window.location;
    window.location = { ...originalLocation, href: "" } as unknown as string & Location;
    render(withProviders(<Login />, { initialEntries: ["/login"] }));
    await userEvent.type(await screen.findByLabelText(/^email$/i), "jane@personal.com");
    await userEvent.click(screen.getByRole("button", { name: /continue/i }));
    expect(await screen.findByRole("button", { name: /provider/i })).toBeInTheDocument();
    expect(window.location.href).toBe("");
    // Token field is still present.
    expect(screen.getByLabelText(/token/i)).toBeInTheDocument();
    window.location = originalLocation as unknown as string & Location;
  });

  it("without ?org=, the shared provider button (revealed after the email fallback) still navigates to /admin/login with no org (regression)", async () => {
    mockFetchByUrl({ authenticated: false, oidc_enabled: true, multi_tenant: true }, {});
    const originalLocation = window.location;
    // @ts-expect-error -- replacing window.location for assertion, restored below.
    delete window.location;
    window.location = { ...originalLocation, href: "" } as unknown as string & Location;
    render(withProviders(<Login />, { initialEntries: ["/login"] }));
    await userEvent.type(await screen.findByLabelText(/^email$/i), "jane@personal.com");
    await userEvent.click(screen.getByRole("button", { name: /continue/i }));
    await userEvent.click(await screen.findByRole("button", { name: /provider/i }));
    expect(window.location.href).toContain("/admin/login");
    expect(window.location.href).not.toContain("org=");
    // The email typed for discovery is still forwarded as login_hint (LUC-76).
    expect(window.location.href).toContain("login_hint=jane%40personal.com");
    // Token form is still present.
    expect(screen.getByLabelText(/token/i)).toBeInTheDocument();
    window.location = originalLocation as unknown as string & Location;
  });
});
