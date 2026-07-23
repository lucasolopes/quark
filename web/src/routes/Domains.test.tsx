import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Domains } from "./Domains";
import { withProviders } from "@/test-utils";

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status });
}

const ME = {
  authenticated: true,
  oidc_enabled: true,
  multi_tenant: true,
  scopes: ["full"],
  tenant_domain_suffix: "quarkus.com.br",
  memberships: [{ tenant_id: 1, name: "W", slug: "w", role: "owner" }],
  current_tenant: 1,
};

const DOMAINS = [
  // Automatic subdomain (empty token, verified) — matches `<slug>.<suffix>`.
  { id: 1, host: "w.quarkus.com.br", status: "verified", created: 1, verified_at: 1, txt_name: "_quark-verify.w.quarkus.com.br", txt_value: "", cname_target: "go.quarkus.com.br", primary: false },
  // Custom domain, pending.
  { id: 2, host: "go.acme.com", status: "pending", created: 2, verified_at: null, txt_name: "_quark-verify.go.acme.com", txt_value: "tok123", cname_target: "go.quarkus.com.br", primary: false },
];

describe("Domains", () => {
  beforeEach(() => { localStorage.removeItem("quark_admin_token"); vi.restoreAllMocks(); });

  it("lists custom + automatic domains and shows the pending DNS instructions", async () => {
    vi.spyOn(globalThis, "fetch").mockImplementation((input) => {
      const url = String(input);
      if (url.includes("/admin/me")) return Promise.resolve(jsonResponse(ME));
      if (url.includes("/admin/domains")) return Promise.resolve(jsonResponse(DOMAINS));
      return Promise.resolve(jsonResponse({}));
    });

    render(withProviders(<Domains />, { withRouter: false }));

    // Automatic subdomain flagged and unremovable.
    expect(await screen.findByText("w.quarkus.com.br")).toBeInTheDocument();
    expect(screen.getByText(/^automatic$/i)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /remove w\.quarkus\.com\.br/i })).not.toBeInTheDocument();

    // Custom domain shows CNAME target + TXT value.
    expect(screen.getAllByText("go.acme.com").length).toBeGreaterThan(0);
    expect(screen.getByText(/go\.quarkus\.com\.br/)).toBeInTheDocument();
    expect(screen.getByText(/tok123/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /verify go\.acme\.com/i })).toBeInTheDocument();
  });

  it("sets a verified custom domain as primary", async () => {
    const verifiedCustom = [
      { id: 1, host: "w.quarkus.com.br", status: "verified", created: 1, verified_at: 1, txt_name: "_quark-verify.w.quarkus.com.br", txt_value: "", cname_target: "go.quarkus.com.br", primary: true },
      { id: 2, host: "go.acme.com", status: "verified", created: 2, verified_at: 2, txt_name: "_quark-verify.go.acme.com", txt_value: "tok123", cname_target: "go.quarkus.com.br", primary: false },
    ];
    const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation((input, init) => {
      const url = String(input);
      if (url.includes("/admin/me")) return Promise.resolve(jsonResponse(ME));
      if (url.includes("/admin/domains/2/primary") && init?.method === "POST") {
        return Promise.resolve(jsonResponse({ ...verifiedCustom[1], primary: true }));
      }
      if (url.includes("/admin/domains")) return Promise.resolve(jsonResponse(verifiedCustom));
      return Promise.resolve(jsonResponse({}));
    });

    render(withProviders(<Domains />, { withRouter: false }));

    // The subdomain is primary; the verified custom offers "Set as primary".
    expect(await screen.findByText(/^primary$/i)).toBeInTheDocument();
    const setPrimaryBtn = screen.getByRole("button", { name: /set go\.acme\.com as the primary domain/i });
    await userEvent.click(setPrimaryBtn);

    await waitFor(() => {
      const post = fetchMock.mock.calls.find(([u, i]) => String(u).includes("/admin/domains/2/primary") && i?.method === "POST");
      expect(post).toBeDefined();
    });
  });

  it("adds a custom domain", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation((input, init) => {
      const url = String(input);
      if (url.includes("/admin/me")) return Promise.resolve(jsonResponse(ME));
      if (url.includes("/admin/domains") && init?.method === "POST") {
        return Promise.resolve(
          jsonResponse({ id: 3, host: "links.acme.com", status: "pending", created: 3, verified_at: null, txt_name: "_quark-verify.links.acme.com", txt_value: "t", cname_target: "backend.quarkus.com.br" }, 201),
        );
      }
      if (url.includes("/admin/domains")) return Promise.resolve(jsonResponse([]));
      return Promise.resolve(jsonResponse({}));
    });

    render(withProviders(<Domains />, { withRouter: false }));
    await screen.findByText(/no custom domains yet/i);

    const openButtons = screen.getAllByRole("button", { name: /add domain/i });
    await userEvent.click(openButtons[0]);
    await userEvent.type(screen.getByLabelText(/^domain$/i), "links.acme.com");
    const submitButtons = screen.getAllByRole("button", { name: /add domain/i });
    await userEvent.click(submitButtons[submitButtons.length - 1]);

    await waitFor(() => {
      const post = fetchMock.mock.calls.find(([, i]) => i?.method === "POST");
      expect(post).toBeDefined();
      const body = JSON.parse(String(post?.[1]?.body)) as { host: string };
      expect(body.host).toBe("links.acme.com");
    });
  });
});
