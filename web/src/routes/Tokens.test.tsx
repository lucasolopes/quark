import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Tokens } from "./Tokens";
import { withProviders } from "@/test-utils";

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status });
}

describe("Tokens", () => {
  beforeEach(() => { localStorage.setItem("quark_admin_token", "s"); vi.restoreAllMocks(); });

  it("lists tokens without leaking the hash", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      jsonResponse({
        tokens: [
          {
            id: 1,
            name: "CI pipeline",
            scopes: ["links_read"],
            rate_limit_per_min: 60,
            created: 1720000000,
            token_hash: "deadbeef-should-never-render",
          },
        ],
      }),
    );
    render(withProviders(<Tokens />, { withRouter: false }));
    expect(await screen.findByText("CI pipeline")).toBeInTheDocument();
    expect(screen.getByText(/links \(read\)/i)).toBeInTheDocument();
    expect(screen.queryByText(/deadbeef/i)).not.toBeInTheDocument();
  });

  it("empty state", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(jsonResponse({ tokens: [] }));
    render(withProviders(<Tokens />, { withRouter: false }));
    expect(await screen.findByText(/no api tokens yet/i)).toBeInTheDocument();
  });

  it("creates a token with the selected scopes and reveals the plaintext once", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation((input, init) => {
      const url = String(input);
      if (url.includes("/admin/tokens") && (!init || init.method === undefined)) {
        return Promise.resolve(jsonResponse({ tokens: [] }));
      }
      if (url.includes("/admin/tokens") && init?.method === "POST") {
        return Promise.resolve(jsonResponse({ id: 5, token: "qtok_supersecretvalue123456789012" }, 201));
      }
      return Promise.resolve(jsonResponse({ tokens: [] }));
    });

    render(withProviders(<Tokens />, { withRouter: false }));
    await screen.findByText(/no api tokens yet/i);

    const openButtons = screen.getAllByRole("button", { name: /create token/i });
    await userEvent.click(openButtons[0]);
    await userEvent.type(screen.getByLabelText(/^name$/i), "CI pipeline");
    await userEvent.click(screen.getByText(/links \(read\)/i));
    await userEvent.click(screen.getByText(/webhooks/i));

    const submitButtons = screen.getAllByRole("button", { name: /create token/i });
    await userEvent.click(submitButtons[submitButtons.length - 1]);

    await waitFor(() => {
      const postCall = fetchMock.mock.calls.find(([, init]) => init?.method === "POST");
      expect(postCall).toBeDefined();
      const body = JSON.parse(String(postCall?.[1]?.body)) as { name: string; scopes: string[] };
      expect(body.name).toBe("CI pipeline");
      expect(body.scopes).toEqual(["links_read", "webhooks"]);
    });

    expect(await screen.findByText(/token created/i)).toBeInTheDocument();
    expect(screen.getByDisplayValue("qtok_supersecretvalue123456789012")).toBeInTheDocument();
  });

  it("revoke asks for confirmation and calls the delete endpoint", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation((_input, init) => {
      if (init?.method === "DELETE") return Promise.resolve(new Response(null, { status: 204 }));
      return Promise.resolve(
        jsonResponse({
          tokens: [{ id: 9, name: "old-token", scopes: ["full"], rate_limit_per_min: null, created: 1720000000 }],
        }),
      );
    });

    render(withProviders(<Tokens />, { withRouter: false }));
    await screen.findByText("old-token");

    await userEvent.click(screen.getByRole("button", { name: /revoke token old-token/i }));
    expect(await screen.findByText(/revoke old-token\?/i)).toBeInTheDocument();

    const confirmButtons = screen.getAllByRole("button", { name: /^revoke$/i });
    await userEvent.click(confirmButtons[confirmButtons.length - 1]);

    await waitFor(() => {
      const deleteCall = fetchMock.mock.calls.find(([, init]) => init?.method === "DELETE");
      expect(deleteCall).toBeDefined();
      expect(String(deleteCall?.[0])).toContain("/admin/tokens/9");
    });
  });
});
