import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { Shell } from "./Shell";
import { withProviders } from "@/test-utils";

function meResponse(body: object) {
  return new Response(JSON.stringify(body), { status: 200 });
}

describe("Shell nav — Members gating", () => {
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

  it("OSS (no memberships field) hides the Members nav item", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(meResponse({ authenticated: true, oidc_enabled: false }));
    render(withProviders(<Shell />, { initialEntries: ["/links"] }));
    await waitFor(() => expect(screen.getByText("quark")).toBeInTheDocument());
    expect(screen.queryByText("Members")).not.toBeInTheDocument();
  });

  it("cloud Owner sees the Members nav item", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      meResponse({
        authenticated: true,
        oidc_enabled: true,
        memberships: [{ tenant_id: 3, name: "Acme", slug: "acme", role: "owner" }],
        current_tenant: 3,
      }),
    );
    render(withProviders(<Shell />, { initialEntries: ["/links"] }));
    expect(await screen.findByText("Members")).toBeInTheDocument();
  });

  it("cloud Admin sees the Members nav item", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      meResponse({
        authenticated: true,
        oidc_enabled: true,
        memberships: [{ tenant_id: 3, name: "Acme", slug: "acme", role: "admin" }],
        current_tenant: 3,
      }),
    );
    render(withProviders(<Shell />, { initialEntries: ["/links"] }));
    expect(await screen.findByText("Members")).toBeInTheDocument();
  });

  it("cloud Member does not see the Members nav item", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      meResponse({
        authenticated: true,
        oidc_enabled: true,
        memberships: [{ tenant_id: 3, name: "Acme", slug: "acme", role: "member" }],
        current_tenant: 3,
      }),
    );
    render(withProviders(<Shell />, { initialEntries: ["/links"] }));
    await waitFor(() => expect(screen.getByText("quark")).toBeInTheDocument());
    expect(screen.queryByText("Members")).not.toBeInTheDocument();
  });

  it("cloud Viewer does not see the Members nav item", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      meResponse({
        authenticated: true,
        oidc_enabled: true,
        memberships: [{ tenant_id: 3, name: "Acme", slug: "acme", role: "viewer" }],
        current_tenant: 3,
      }),
    );
    render(withProviders(<Shell />, { initialEntries: ["/links"] }));
    await waitFor(() => expect(screen.getByText("quark")).toBeInTheDocument());
    expect(screen.queryByText("Members")).not.toBeInTheDocument();
  });
});
