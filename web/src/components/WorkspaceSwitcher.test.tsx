import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { withProviders } from "@/test-utils";
import { WorkspaceSwitcher } from "./WorkspaceSwitcher";

function me(body: object) { return new Response(JSON.stringify(body), { status: 200 }); }
const cloudMe = {
  authenticated: true, oidc_enabled: true, current_tenant: 1,
  memberships: [
    // The wire format is serde snake_case, so the server sends lowercase roles.
    { tenant_id: 1, name: "Acme", slug: "acme", role: "owner" },
    { tenant_id: 2, name: "Beta", slug: "beta", role: "member" },
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

  it("offers deletion when the current workspace's role is owner", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(me(cloudMe));
    render(withProviders(<WorkspaceSwitcher />));
    await userEvent.click(await screen.findByRole("button", { name: /acme/i }));
    expect(await screen.findByText(/delete workspace/i)).toBeInTheDocument();
  });

  it("does not offer deletion to a non-owner of the current workspace", async () => {
    // Same memberships, but the session sits on Beta, where the user is a Member.
    vi.spyOn(globalThis, "fetch").mockResolvedValue(me({ ...cloudMe, current_tenant: 2 }));
    render(withProviders(<WorkspaceSwitcher />));
    await userEvent.click(await screen.findByRole("button", { name: /beta/i }));
    expect(await screen.findByText(/create workspace/i)).toBeInTheDocument();
    expect(screen.queryByText(/delete workspace/i)).not.toBeInTheDocument();
  });
});
