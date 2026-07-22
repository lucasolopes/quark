import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Members } from "./Members";
import { withProviders } from "@/test-utils";

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status });
}

describe("Members", () => {
  beforeEach(() => { localStorage.setItem("quark_admin_token", "s"); vi.restoreAllMocks(); });

  it("lists pending invites", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      jsonResponse([
        { id: 1, email: "ana@example.com", role: "admin", expires: 1720100000, created: 1720000000 },
      ]),
    );
    render(withProviders(<Members />, { withRouter: false }));
    expect(await screen.findByText("ana@example.com")).toBeInTheDocument();
    expect(screen.getByText("Admin")).toBeInTheDocument();
  });

  it("shows a dedicated permission message on 403", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 403 }));
    render(withProviders(<Members />, { withRouter: false }));
    expect(await screen.findByText(/don't have permission to view members/i)).toBeInTheDocument();
  });

  it("empty state", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(jsonResponse([]));
    render(withProviders(<Members />, { withRouter: false }));
    expect(await screen.findByText(/no invites yet/i)).toBeInTheDocument();
  });

  it("invites a member with a lowercase role and shows the copyable accept link", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation((input, init) => {
      const url = String(input);
      if (url.includes("/admin/invites") && (!init || init.method === undefined)) {
        return Promise.resolve(jsonResponse([]));
      }
      if (url.includes("/admin/invites") && init?.method === "POST") {
        return Promise.resolve(
          jsonResponse(
            { id: 5, token: "tok_abc123", email: "bob@example.com", role: "member", expires: 1720100000 },
            201,
          ),
        );
      }
      return Promise.resolve(jsonResponse([]));
    });

    render(withProviders(<Members />, { withRouter: false }));
    await screen.findByText(/no invites yet/i);

    const openButtons = screen.getAllByRole("button", { name: /invite member/i });
    await userEvent.click(openButtons[0]);
    await userEvent.type(screen.getByLabelText(/^email$/i), "bob@example.com");
    // Role select defaults to "member" — leave as-is to test the default lowercase value.

    const submitButtons = screen.getAllByRole("button", { name: /send invite/i });
    await userEvent.click(submitButtons[submitButtons.length - 1]);

    await waitFor(() => {
      const postCall = fetchMock.mock.calls.find(([, init]) => init?.method === "POST");
      expect(postCall).toBeDefined();
      const body = JSON.parse(String(postCall?.[1]?.body)) as { email: string; role: string };
      expect(body.email).toBe("bob@example.com");
      expect(body.role).toBe("member");
    });

    expect(await screen.findByDisplayValue(/\/invite\/tok_abc123$/)).toBeInTheDocument();
  });

  it("in SSO-provisioning mode confirms an email was sent instead of a copyable link", async () => {
    // No break-glass token, so `useMe` runs and reports the IdP-provisioning flag.
    localStorage.removeItem("quark_admin_token");
    vi.spyOn(globalThis, "fetch").mockImplementation((input, init) => {
      const url = String(input);
      if (url.includes("/admin/me")) {
        return Promise.resolve(
          jsonResponse({
            authenticated: true,
            oidc_enabled: true,
            multi_tenant: true,
            sso_provisioning: true,
            memberships: [{ tenant_id: 1, name: "W", slug: "w", role: "owner" }],
            current_tenant: 1,
          }),
        );
      }
      if (url.includes("/admin/invites") && init?.method === "POST") {
        return Promise.resolve(
          jsonResponse(
            { id: 5, token: "tok_abc123", email: "bob@example.com", role: "member", expires: 1720100000 },
            201,
          ),
        );
      }
      return Promise.resolve(jsonResponse([]));
    });

    render(withProviders(<Members />, { withRouter: false }));
    await screen.findByText(/no invites yet/i);

    const openButtons = screen.getAllByRole("button", { name: /invite member/i });
    await userEvent.click(openButtons[0]);
    await userEvent.type(screen.getByLabelText(/^email$/i), "bob@example.com");

    const submitButtons = screen.getAllByRole("button", { name: /send invite/i });
    await userEvent.click(submitButtons[submitButtons.length - 1]);

    expect(await screen.findByText(/invite email sent/i)).toBeInTheDocument();
    expect(screen.getByText(/emailed bob@example\.com/i)).toBeInTheDocument();
    // The dead `/invite/<token>` link must not be offered under IdP provisioning.
    expect(screen.queryByDisplayValue(/\/invite\//)).not.toBeInTheDocument();
  });

  it("revoke asks for confirmation and calls the delete endpoint", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation((_input, init) => {
      if (init?.method === "DELETE") return Promise.resolve(new Response(null, { status: 204 }));
      return Promise.resolve(
        jsonResponse([{ id: 9, email: "old@example.com", role: "viewer", expires: 1720100000, created: 1720000000 }]),
      );
    });

    render(withProviders(<Members />, { withRouter: false }));
    await screen.findByText("old@example.com");

    await userEvent.click(screen.getByRole("button", { name: /revoke old@example\.com/i }));
    expect(await screen.findByText(/revoke this invite\?/i)).toBeInTheDocument();

    const confirmButtons = screen.getAllByRole("button", { name: /^revoke$/i });
    await userEvent.click(confirmButtons[confirmButtons.length - 1]);

    await waitFor(() => {
      const deleteCall = fetchMock.mock.calls.find(([, init]) => init?.method === "DELETE");
      expect(deleteCall).toBeDefined();
      expect(String(deleteCall?.[0])).toContain("/admin/invites/9");
    });
  });
});
