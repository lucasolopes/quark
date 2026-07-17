import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Login } from "./Login";
import { withProviders } from "@/test-utils";

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

  it("shows the provider button only when OIDC is enabled", async () => {
    vi.spyOn(globalThis, "fetch").mockImplementation(() =>
      Promise.resolve(
        new Response(JSON.stringify({ authenticated: false, oidc_enabled: true }), { status: 200 }),
      ),
    );
    render(withProviders(<Login />));
    expect(await screen.findByRole("button", { name: /provider/i })).toBeInTheDocument();
  });

  it("hides the provider button when OIDC is disabled", async () => {
    vi.spyOn(globalThis, "fetch").mockImplementation(() =>
      Promise.resolve(
        new Response(JSON.stringify({ authenticated: false, oidc_enabled: false }), { status: 200 }),
      ),
    );
    render(withProviders(<Login />));
    // Give the mount probe time to resolve.
    await screen.findByLabelText(/token/i);
    expect(screen.queryByRole("button", { name: /provider/i })).not.toBeInTheDocument();
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

  it("with ?org= and OIDC enabled, shows the org header and a per-tenant sign-in button", async () => {
    vi.spyOn(globalThis, "fetch").mockImplementation(() =>
      Promise.resolve(
        new Response(JSON.stringify({ authenticated: false, oidc_enabled: true }), { status: 200 }),
      ),
    );
    render(withProviders(<Login />, { initialEntries: ["/login?org=acme"] }));
    expect(await screen.findByText(/organization: acme/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /sign in to acme/i })).toBeInTheDocument();
    // Token form stays present alongside the org-aware provider button.
    expect(screen.getByLabelText(/token/i)).toBeInTheDocument();
  });

  it("clicking the per-tenant button navigates to /admin/login?org=acme", async () => {
    vi.spyOn(globalThis, "fetch").mockImplementation(() =>
      Promise.resolve(
        new Response(JSON.stringify({ authenticated: false, oidc_enabled: true }), { status: 200 }),
      ),
    );
    const originalLocation = window.location;
    // @ts-expect-error -- replacing window.location for assertion, restored below.
    delete window.location;
    window.location = { ...originalLocation, href: "" } as Location;
    render(withProviders(<Login />, { initialEntries: ["/login?org=acme"] }));
    await userEvent.click(await screen.findByRole("button", { name: /sign in to acme/i }));
    expect(window.location.href).toContain("/admin/login?org=acme");
    window.location = originalLocation;
  });

  it("without ?org=, the shared provider button still navigates to /admin/login with no org (regression)", async () => {
    vi.spyOn(globalThis, "fetch").mockImplementation(() =>
      Promise.resolve(
        new Response(JSON.stringify({ authenticated: false, oidc_enabled: true }), { status: 200 }),
      ),
    );
    const originalLocation = window.location;
    // @ts-expect-error -- replacing window.location for assertion, restored below.
    delete window.location;
    window.location = { ...originalLocation, href: "" } as Location;
    render(withProviders(<Login />, { initialEntries: ["/login"] }));
    await userEvent.click(await screen.findByRole("button", { name: /provider/i }));
    expect(window.location.href).toContain("/admin/login");
    expect(window.location.href).not.toContain("org=");
    // Token form is still present.
    expect(screen.getByLabelText(/token/i)).toBeInTheDocument();
    window.location = originalLocation;
  });
});
