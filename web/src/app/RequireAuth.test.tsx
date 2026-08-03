import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
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




});
