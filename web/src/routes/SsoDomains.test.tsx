import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { SsoDomains } from "./SsoDomains";
import { withProviders } from "@/test-utils";

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status });
}

const CLOUD_ME = {
  authenticated: true,
  oidc_enabled: true,
  memberships: [{ tenant_id: 3, name: "Acme", slug: "acme", role: "owner" }],
  current_tenant: 3,
};

const OSS_ME = { authenticated: true, oidc_enabled: false };

const PENDING_DOMAIN = {
  id: 1,
  domain: "acme.com",
  status: "pending",
  created: 1720000000,
  verified_at: null,
  txt_name: "_quark-sso.acme.com",
  txt_value: "tok_abc123",
};

const VERIFIED_DOMAIN = {
  id: 2,
  domain: "widgets.io",
  status: "verified",
  created: 1720000000,
  verified_at: 1720100000,
  txt_name: "_quark-sso.widgets.io",
  txt_value: "tok_def456",
};

/** Routes fetch by URL/method: `/admin/me`, `/admin/oidc-config`, `/admin/sso-domains[...]`. */
function mockFetch(opts: {
  me?: object;
  oidcConfigured?: boolean;
  domains?: object[];
  onCreate?: () => object;
  onVerify?: (id: string) => object;
  onDelete?: () => void;
}) {
  const { me = CLOUD_ME, oidcConfigured = true, domains = [] } = opts;
  return vi.spyOn(globalThis, "fetch").mockImplementation((input, init) => {
    const url = String(input);
    const method = init?.method ?? "GET";
    if (url.includes("/admin/me")) return Promise.resolve(jsonResponse(me));
    if (url.includes("/admin/oidc-config")) {
      return Promise.resolve(oidcConfigured ? jsonResponse({}) : new Response("", { status: 404 }));
    }
    if (url.includes("/admin/sso-domains")) {
      if (method === "POST" && url.endsWith("/verify")) {
        const id = url.match(/sso-domains\/(\d+)\/verify/)?.[1] ?? "0";
        return Promise.resolve(jsonResponse(opts.onVerify ? opts.onVerify(id) : VERIFIED_DOMAIN));
      }
      if (method === "POST") {
        return Promise.resolve(jsonResponse(opts.onCreate ? opts.onCreate() : PENDING_DOMAIN));
      }
      if (method === "DELETE") {
        opts.onDelete?.();
        return Promise.resolve(new Response(null, { status: 204 }));
      }
      return Promise.resolve(jsonResponse(domains));
    }
    return Promise.resolve(jsonResponse({}));
  });
}

describe("SsoDomains", () => {
  // No admin token: `useMe` (which the gate depends on) is disabled while a
  // break-glass token is present, so these tests simulate a cookie session.
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

  it("OSS (no memberships) renders nothing", async () => {
    mockFetch({ me: OSS_ME });
    const { container } = render(withProviders(<SsoDomains />, { withRouter: false }));
    await waitFor(() => expect(container).toBeEmptyDOMElement());
  });

  it("cloud without an SSO provider configured shows the not-configured message, no domain list", async () => {
    mockFetch({ oidcConfigured: false, domains: [PENDING_DOMAIN] });
    render(withProviders(<SsoDomains />, { withRouter: false }));
    expect(await screen.findByText(/set up an sso provider/i)).toBeInTheDocument();
    expect(screen.queryByText("acme.com")).not.toBeInTheDocument();
  });

  it("cloud with SSO configured lists a pending and a verified domain, showing the TXT record only for the pending one", async () => {
    mockFetch({ domains: [PENDING_DOMAIN, VERIFIED_DOMAIN] });
    render(withProviders(<SsoDomains />, { withRouter: false }));

    expect(await screen.findByText("acme.com")).toBeInTheDocument();
    expect(screen.getByText("widgets.io")).toBeInTheDocument();
    expect(screen.getByText("Pending")).toBeInTheDocument();
    expect(screen.getByText("Verified")).toBeInTheDocument();

    // TXT record shown for the pending domain only.
    expect(screen.getByText("_quark-sso.acme.com")).toBeInTheDocument();
    expect(screen.getByText("tok_abc123")).toBeInTheDocument();
    expect(screen.queryByText("_quark-sso.widgets.io")).not.toBeInTheDocument();
  });

  it("empty state when there are no domains yet", async () => {
    mockFetch({ domains: [] });
    render(withProviders(<SsoDomains />, { withRouter: false }));
    expect(await screen.findByText(/no domains yet/i)).toBeInTheDocument();
  });

  it("clicking verify calls the verify endpoint and refetches", async () => {
    let served: object[] = [PENDING_DOMAIN];
    const fetchMock = mockFetch({ domains: [PENDING_DOMAIN] });
    // Override the list handler to reflect `served` across refetches.
    fetchMock.mockImplementation((input) => {
      const url = String(input);
      if (url.includes("/admin/me")) return Promise.resolve(jsonResponse(CLOUD_ME));
      if (url.includes("/admin/oidc-config")) return Promise.resolve(jsonResponse({}));
      if (url.includes("/verify")) {
        served = [{ ...PENDING_DOMAIN, status: "verified", verified_at: 1720200000 }];
        return Promise.resolve(jsonResponse(served[0]));
      }
      if (url.includes("/admin/sso-domains")) return Promise.resolve(jsonResponse(served));
      return Promise.resolve(jsonResponse({}));
    });

    render(withProviders(<SsoDomains />, { withRouter: false }));
    await screen.findByText("acme.com");

    await userEvent.click(screen.getByRole("button", { name: /verify acme\.com/i }));

    await waitFor(() => {
      const verifyCall = fetchMock.mock.calls.find(([u]) => String(u).includes("/verify"));
      expect(verifyCall).toBeDefined();
      expect(String(verifyCall?.[0])).toContain("/admin/sso-domains/1/verify");
    });
  });

  it("add form calls createSsoDomain with the trimmed domain", async () => {
    const fetchMock = mockFetch({ domains: [] });
    render(withProviders(<SsoDomains />, { withRouter: false }));
    await screen.findByText(/no domains yet/i);

    const openButtons = screen.getAllByRole("button", { name: /add domain/i });
    await userEvent.click(openButtons[0]);
    await userEvent.type(screen.getByLabelText(/^domain$/i), " acme.com ");

    const submitButtons = screen.getAllByRole("button", { name: /^add domain$/i });
    await userEvent.click(submitButtons[submitButtons.length - 1]);

    await waitFor(() => {
      const postCall = fetchMock.mock.calls.find(([u, init]) => String(u).endsWith("/admin/sso-domains") && init?.method === "POST");
      expect(postCall).toBeDefined();
      const body = JSON.parse(String(postCall?.[1]?.body)) as { domain: string };
      expect(body.domain).toBe("acme.com");
    });
  });

  it("remove asks for confirmation and calls the delete endpoint", async () => {
    const onDelete = vi.fn();
    const fetchMock = mockFetch({ domains: [PENDING_DOMAIN], onDelete });
    render(withProviders(<SsoDomains />, { withRouter: false }));
    await screen.findByText("acme.com");

    await userEvent.click(screen.getByRole("button", { name: /remove acme\.com/i }));
    expect(await screen.findByText(/remove this domain\?/i)).toBeInTheDocument();

    const confirmButtons = screen.getAllByRole("button", { name: /^remove$/i });
    await userEvent.click(confirmButtons[confirmButtons.length - 1]);

    await waitFor(() => {
      const deleteCall = fetchMock.mock.calls.find(([, init]) => init?.method === "DELETE");
      expect(deleteCall).toBeDefined();
      expect(String(deleteCall?.[0])).toContain("/admin/sso-domains/1");
      expect(onDelete).toHaveBeenCalled();
    });
  });
});
