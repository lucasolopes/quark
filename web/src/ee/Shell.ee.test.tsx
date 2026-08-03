import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { Shell } from "@/app/Shell";
import { withProviders } from "@/test-utils";

function meResponse(body: object) {
  return new Response(JSON.stringify(body), { status: 200 });
}

describe("Shell (Enterprise)", () => {
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

  it("still renders the workspace switcher, now inside the sidebar (cloud)", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      meResponse({
        authenticated: true,
        oidc_enabled: true,
        memberships: [{ tenant_id: 3, name: "Acme", slug: "acme", role: "owner" }],
        current_tenant: 3,
      }),
    );
    render(withProviders(<Shell />, { initialEntries: ["/links"] }));
    expect(await screen.findByRole("button", { name: /acme/i })).toBeInTheDocument();
  });
});
