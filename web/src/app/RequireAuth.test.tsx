import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { RequireAuth } from "./RequireAuth";
import { withProviders } from "@/test-utils";

function meResponse(body: object) {
  return new Response(JSON.stringify(body), { status: 200 });
}
const child = <div>APP CONTENT</div>;

describe("RequireAuth workspace gate", () => {
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

  it("OSS (no memberships field) renders the app", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(meResponse({ authenticated: true, oidc_enabled: false }));
    render(withProviders(<RequireAuth>{child}</RequireAuth>));
    expect(await screen.findByText("APP CONTENT")).toBeInTheDocument();
  });

  it("cloud with a current workspace renders the app", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      meResponse({ authenticated: true, oidc_enabled: true, memberships: [{ tenant_id: 3, name: "Acme", slug: "acme", role: "Owner" }], current_tenant: 3 }),
    );
    render(withProviders(<RequireAuth>{child}</RequireAuth>));
    expect(await screen.findByText("APP CONTENT")).toBeInTheDocument();
  });

  it("cloud with zero memberships shows onboarding, not the app", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      meResponse({ authenticated: true, oidc_enabled: true, memberships: [], current_tenant: null }),
    );
    render(withProviders(<RequireAuth>{child}</RequireAuth>));
    expect(await screen.findByText(/create your workspace/i)).toBeInTheDocument();
    expect(screen.queryByText("APP CONTENT")).not.toBeInTheDocument();
  });

  it("cloud with exactly one membership and no current workspace auto-switches to it", async () => {
    const spy = vi.spyOn(globalThis, "fetch").mockImplementation((url) => {
      if (String(url).includes("/admin/workspace/switch")) return Promise.resolve(new Response("", { status: 200 }));
      // First /admin/me: no current; after the switch invalidates, still fine to return current set.
      return Promise.resolve(meResponse({ authenticated: true, oidc_enabled: true, memberships: [{ tenant_id: 9, name: "Solo", slug: "solo", role: "Owner" }], current_tenant: null }));
    });
    render(withProviders(<RequireAuth>{child}</RequireAuth>));
    await waitFor(() => {
      expect(spy.mock.calls.some((c) => String(c[0]).includes("/admin/workspace/switch") && JSON.parse(String(c[1]?.body)).tenant_id === 9)).toBe(true);
    });
  });

  it("falls through to onboarding when the single-membership auto-switch fails", async () => {
    // The switch is rate-limited (429): the gate must not strand the user on the
    // spinner — it shows the chooser, where the workspace is a clickable retry.
    vi.spyOn(globalThis, "fetch").mockImplementation((url) => {
      if (String(url).includes("/admin/workspace/switch")) return Promise.resolve(new Response("", { status: 429 }));
      return Promise.resolve(meResponse({ authenticated: true, oidc_enabled: true, memberships: [{ tenant_id: 9, name: "Solo", slug: "solo", role: "Owner" }], current_tenant: null }));
    });
    render(withProviders(<RequireAuth>{child}</RequireAuth>));
    expect(await screen.findByText("Solo")).toBeInTheDocument();
    expect(screen.queryByText("APP CONTENT")).not.toBeInTheDocument();
  });

  it("cloud with two memberships and no current shows the chooser", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      meResponse({ authenticated: true, oidc_enabled: true, memberships: [
        { tenant_id: 1, name: "Acme", slug: "acme", role: "Owner" },
        { tenant_id: 2, name: "Beta", slug: "beta", role: "Member" },
      ], current_tenant: null }),
    );
    render(withProviders(<RequireAuth>{child}</RequireAuth>));
    expect(await screen.findByText(/choose a workspace/i)).toBeInTheDocument();
    expect(screen.getByText("Acme")).toBeInTheDocument();
    expect(screen.getByText("Beta")).toBeInTheDocument();
  });
});
