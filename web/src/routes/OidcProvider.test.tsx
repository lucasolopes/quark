import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { OidcProvider } from "./OidcProvider";
import { withProviders } from "@/test-utils";

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status });
}

const ME = {
  authenticated: true,
  oidc_enabled: true,
  multi_tenant: true,
  scopes: ["full"],
  memberships: [{ tenant_id: 1, name: "W", slug: "w", role: "owner" }],
  current_tenant: 1,
};

const CONFIG = {
  issuer: "https://acme.okta.com",
  client_id: "quark-acme",
  scopes: ["openid", "profile", "email"],
  admin_claim: "groups",
  admin_value: "acme-admins",
  readonly_value: "acme-viewers",
  member_value: "acme-members",
  required_value: null,
  post_login_url: null,
  client_secret_set: true,
};

describe("OidcProvider", () => {
  // No break-glass token so `useMe` runs and reports the cloud/admin scopes.
  beforeEach(() => { localStorage.removeItem("quark_admin_token"); vi.restoreAllMocks(); });

  it("shows the configured provider and saves an edit with a blank secret (preserve)", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation((input, init) => {
      const url = String(input);
      if (url.includes("/admin/me")) return Promise.resolve(jsonResponse(ME));
      if (url.includes("/admin/oidc-config") && init?.method === "PUT") {
        return Promise.resolve(jsonResponse(CONFIG));
      }
      if (url.includes("/admin/oidc-config")) return Promise.resolve(jsonResponse(CONFIG));
      return Promise.resolve(jsonResponse({}));
    });

    render(withProviders(<OidcProvider />, { withRouter: false }));

    expect(await screen.findByDisplayValue("https://acme.okta.com")).toBeInTheDocument();
    expect(screen.getByText(/secret is on file/i)).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /save provider/i }));

    await waitFor(() => {
      const put = fetchMock.mock.calls.find(([, i]) => i?.method === "PUT");
      expect(put).toBeDefined();
      const body = JSON.parse(String(put?.[1]?.body)) as { issuer: string; client_secret: string };
      expect(body.issuer).toBe("https://acme.okta.com");
      // Left blank on an edit → empty string, which the backend treats as "keep the stored secret".
      expect(body.client_secret).toBe("");
    });
  });

  it("shows the empty state when no provider is configured (404)", async () => {
    vi.spyOn(globalThis, "fetch").mockImplementation((input) => {
      const url = String(input);
      if (url.includes("/admin/me")) return Promise.resolve(jsonResponse(ME));
      if (url.includes("/admin/oidc-config")) return Promise.resolve(new Response("", { status: 404 }));
      return Promise.resolve(jsonResponse({}));
    });

    render(withProviders(<OidcProvider />, { withRouter: false }));

    expect(await screen.findByText(/no sso provider configured/i)).toBeInTheDocument();
  });
});
