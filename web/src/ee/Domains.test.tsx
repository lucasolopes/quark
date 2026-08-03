import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
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
    expect(screen.getByRole("button", { name: /remove go\.acme\.com/i })).toBeInTheDocument();
  });

  it("renders each domain as a card with a status dot colored per verification state", async () => {
    vi.spyOn(globalThis, "fetch").mockImplementation((input) => {
      const url = String(input);
      if (url.includes("/admin/me")) return Promise.resolve(jsonResponse(ME));
      if (url.includes("/admin/domains")) return Promise.resolve(jsonResponse(DOMAINS));
      return Promise.resolve(jsonResponse({}));
    });

    render(withProviders(<Domains />, { withRouter: false }));

    const cards = await screen.findAllByTestId("domain-card");
    expect(cards).toHaveLength(2);
    // Semantic list: each card is a real <li> so the stack reads as a list.
    expect(screen.getAllByRole("listitem")).toHaveLength(2);

    // `queryAllByText` (not `queryByText`): the pending card repeats its own
    // host inside the DNS instructions text, so a single-match query would
    // throw "multiple elements found" for that card.
    const verifiedCard = cards.find((c) => within(c).queryAllByText("w.quarkus.com.br").length > 0);
    const pendingCard = cards.find((c) => within(c).queryAllByText("go.acme.com").length > 0);
    if (!verifiedCard || !pendingCard) throw new Error("expected a verified card and a pending card");

    // verified = bg-primary, pending = bg-muted-foreground (LUC-82 v2 status semantics).
    expect(within(verifiedCard).getByTestId("domain-status-dot")).toHaveClass("bg-primary");
    expect(within(verifiedCard).getByText(/^verified$/i)).toBeInTheDocument();
    expect(within(pendingCard).getByTestId("domain-status-dot")).toHaveClass("bg-muted-foreground");
    expect(within(pendingCard).getByText(/^pending$/i)).toBeInTheDocument();
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

    // The badge is scoped to the primary domain's own card, not the other one.
    const cards = screen.getAllByTestId("domain-card");
    const nonPrimaryCard = cards.find((c) => within(c).queryAllByText("go.acme.com").length > 0);
    if (!nonPrimaryCard) throw new Error("expected go.acme.com's card");
    expect(within(nonPrimaryCard).queryByText(/^primary$/i)).not.toBeInTheDocument();

    const setPrimaryBtn = screen.getByRole("button", { name: /set go\.acme\.com as the primary domain/i });
    await userEvent.click(setPrimaryBtn);

    await waitFor(() => {
      const post = fetchMock.mock.calls.find(([u, i]) => String(u).includes("/admin/domains/2/primary") && i?.method === "POST");
      expect(post).toBeDefined();
    });
  });

  it("removes a custom domain after confirming", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation((input, init) => {
      const url = String(input);
      if (url.includes("/admin/me")) return Promise.resolve(jsonResponse(ME));
      if (url.includes("/admin/domains/2") && init?.method === "DELETE") {
        return Promise.resolve(new Response(null, { status: 204 }));
      }
      if (url.includes("/admin/domains")) return Promise.resolve(jsonResponse(DOMAINS));
      return Promise.resolve(jsonResponse({}));
    });

    render(withProviders(<Domains />, { withRouter: false }));
    const removeBtn = await screen.findByRole("button", { name: /remove go\.acme\.com/i });

    await userEvent.click(removeBtn);
    const dialog = await screen.findByRole("alertdialog");
    await userEvent.click(within(dialog).getByRole("button", { name: /^remove$/i }));

    await waitFor(() => {
      const del = fetchMock.mock.calls.find(([u, i]) => String(u).includes("/admin/domains/2") && i?.method === "DELETE");
      expect(del).toBeDefined();
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
