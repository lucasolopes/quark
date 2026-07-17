import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { withProviders } from "@/test-utils";
import { WorkspaceSwitcher } from "./WorkspaceSwitcher";

function me(body: object) { return new Response(JSON.stringify(body), { status: 200 }); }
const cloudMe = {
  authenticated: true, oidc_enabled: true, current_tenant: 1,
  memberships: [
    { tenant_id: 1, name: "Acme", slug: "acme", role: "Owner" },
    { tenant_id: 2, name: "Beta", slug: "beta", role: "Member" },
  ],
};

describe("WorkspaceSwitcher", () => {
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

  it("renders nothing in OSS (no memberships field)", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(me({ authenticated: true, oidc_enabled: false }));
    const { container } = render(withProviders(<WorkspaceSwitcher />));
    // Give the me() query time to resolve, then assert empty.
    await waitFor(() => expect(container).toBeEmptyDOMElement());
  });

  it("shows the current workspace and lists the others; selecting one switches", async () => {
    const spy = vi.spyOn(globalThis, "fetch").mockImplementation((url) =>
      String(url).includes("/admin/workspace/switch")
        ? Promise.resolve(new Response("", { status: 200 }))
        : Promise.resolve(me(cloudMe)),
    );
    render(withProviders(<WorkspaceSwitcher />));
    await userEvent.click(await screen.findByRole("button", { name: /acme/i }));
    await userEvent.click(await screen.findByText("Beta"));
    await waitFor(() => {
      expect(spy.mock.calls.some((c) => String(c[0]).includes("/admin/workspace/switch") && JSON.parse(String(c[1]?.body)).tenant_id === 2)).toBe(true);
    });
  });
});
