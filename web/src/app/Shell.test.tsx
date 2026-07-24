import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Route, Routes, useSearchParams } from "react-router-dom";
import { Shell } from "./Shell";
import { withProviders } from "@/test-utils";

function meResponse(body: object) {
  return new Response(JSON.stringify(body), { status: 200 });
}

/** Renders the params of whatever route matched, so navigation via `navigate()` is observable. */
function SearchParamsProbe() {
  const [params] = useSearchParams();
  return <div data-testid="probe">{params.toString()}</div>;
}

/**
 * Mounts `Shell` as the layout for `/links` and `/analytics` — mirroring the
 * real router, where Shell wraps every child route via one layout element —
 * each child rendering `SearchParamsProbe`, so a `navigate(...)` or
 * `setSearchParams(...)` call from inside `Shell` is observable via whichever
 * probe ends up matching after the call.
 */
function renderShellWithProbe(initialEntries: string[]) {
  return render(
    withProviders(
      <Routes>
        <Route path="/" element={<Shell />}>
          <Route path="links" element={<SearchParamsProbe />} />
          <Route path="analytics" element={<SearchParamsProbe />} />
        </Route>
      </Routes>,
      { initialEntries },
    ),
  );
}

describe("Shell nav — Members gating", () => {
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

  it("OSS (no memberships field) hides the Members nav item", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(meResponse({ authenticated: true, oidc_enabled: false }));
    render(withProviders(<Shell />, { initialEntries: ["/links"] }));
    await waitFor(() => expect(screen.getByText("quark")).toBeInTheDocument());
    expect(screen.queryByText("Members")).not.toBeInTheDocument();
  });

  it("cloud Owner sees the Members nav item", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      meResponse({
        authenticated: true,
        oidc_enabled: true,
        memberships: [{ tenant_id: 3, name: "Acme", slug: "acme", role: "owner" }],
        current_tenant: 3,
      }),
    );
    render(withProviders(<Shell />, { initialEntries: ["/links"] }));
    expect(await screen.findByText("Members")).toBeInTheDocument();
  });

  it("cloud Admin sees the Members nav item", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      meResponse({
        authenticated: true,
        oidc_enabled: true,
        memberships: [{ tenant_id: 3, name: "Acme", slug: "acme", role: "admin" }],
        current_tenant: 3,
      }),
    );
    render(withProviders(<Shell />, { initialEntries: ["/links"] }));
    expect(await screen.findByText("Members")).toBeInTheDocument();
  });

  it("cloud Member does not see the Members nav item", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      meResponse({
        authenticated: true,
        oidc_enabled: true,
        memberships: [{ tenant_id: 3, name: "Acme", slug: "acme", role: "member" }],
        current_tenant: 3,
      }),
    );
    render(withProviders(<Shell />, { initialEntries: ["/links"] }));
    await waitFor(() => expect(screen.getByText("quark")).toBeInTheDocument());
    expect(screen.queryByText("Members")).not.toBeInTheDocument();
  });

  it("cloud Viewer does not see the Members nav item", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      meResponse({
        authenticated: true,
        oidc_enabled: true,
        memberships: [{ tenant_id: 3, name: "Acme", slug: "acme", role: "viewer" }],
        current_tenant: 3,
      }),
    );
    render(withProviders(<Shell />, { initialEntries: ["/links"] }));
    await waitFor(() => expect(screen.getByText("quark")).toBeInTheDocument());
    expect(screen.queryByText("Members")).not.toBeInTheDocument();
  });
});

describe("Shell v2 — sidebar + topbar", () => {
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

  it("shows the global search input", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(meResponse({ authenticated: true, oidc_enabled: false }));
    render(withProviders(<Shell />, { initialEntries: ["/links"] }));
    await waitFor(() => expect(screen.getByText("quark")).toBeInTheDocument());
    expect(screen.getByPlaceholderText("Search links…")).toBeInTheDocument();
  });

  it("shows the New link button", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(meResponse({ authenticated: true, oidc_enabled: false }));
    render(withProviders(<Shell />, { initialEntries: ["/links"] }));
    await waitFor(() => expect(screen.getByText("quark")).toBeInTheDocument());
    expect(screen.getByRole("button", { name: "New link" })).toBeInTheDocument();
  });

  it("shows the logout button in the sidebar footer, still findable by role/name", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(meResponse({ authenticated: true, oidc_enabled: false }));
    render(withProviders(<Shell />, { initialEntries: ["/links"] }));
    await waitFor(() => expect(screen.getByText("quark")).toBeInTheDocument());
    expect(screen.getByRole("button", { name: /log ?out|sair/i })).toBeInTheDocument();
  });

  it("derives the sidebar avatar initials from the signed-in display name", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      meResponse({ authenticated: true, oidc_enabled: true, display: "Lucas Olopes" }),
    );
    render(withProviders(<Shell />, { initialEntries: ["/links"] }));
    expect(await screen.findByText("LO")).toBeInTheDocument();
    expect(screen.getByText("Lucas Olopes")).toBeInTheDocument();
  });

  it("on another screen, pressing Enter in the search box navigates to /links?q=<term>", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(meResponse({ authenticated: true, oidc_enabled: false }));
    renderShellWithProbe(["/analytics"]);
    await waitFor(() => expect(screen.getByText("quark")).toBeInTheDocument());
    await userEvent.type(screen.getByPlaceholderText("Search links…"), "summer promo{enter}");
    await waitFor(() => expect(screen.getByTestId("probe").textContent).toBe("q=summer+promo"));
  });

  it("on another screen, pressing Enter with an empty search does not navigate", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(meResponse({ authenticated: true, oidc_enabled: false }));
    renderShellWithProbe(["/analytics"]);
    await waitFor(() => expect(screen.getByText("quark")).toBeInTheDocument());
    await userEvent.type(screen.getByPlaceholderText("Search links…"), "{enter}");
    expect(screen.getByTestId("probe").textContent).toBe("");
  });

  it("on /links, typing live-updates the q param after a debounce (no Enter needed)", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(meResponse({ authenticated: true, oidc_enabled: false }));
    renderShellWithProbe(["/links"]);
    await waitFor(() => expect(screen.getByText("quark")).toBeInTheDocument());
    await userEvent.type(screen.getByPlaceholderText("Search links…"), "summer promo");
    await waitFor(() => expect(screen.getByTestId("probe").textContent).toBe("q=summer+promo"));
  });

  it("on /links, clearing the box clears the q param", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(meResponse({ authenticated: true, oidc_enabled: false }));
    renderShellWithProbe(["/links?q=promo"]);
    const input = await screen.findByPlaceholderText("Search links…");
    expect(input).toHaveValue("promo");
    await userEvent.clear(input);
    await waitFor(() => expect(screen.getByTestId("probe").textContent).toBe(""));
  });

  it("reflects ?q= already in the URL when landing on /links (e.g. via back/forward)", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(meResponse({ authenticated: true, oidc_enabled: false }));
    renderShellWithProbe(["/links?q=promo"]);
    await waitFor(() => expect(screen.getByPlaceholderText("Search links…")).toHaveValue("promo"));
  });

  it("clicking New link navigates to /links?new=1", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(meResponse({ authenticated: true, oidc_enabled: false }));
    renderShellWithProbe(["/links"]);
    await waitFor(() => expect(screen.getByText("quark")).toBeInTheDocument());
    await userEvent.click(screen.getByRole("button", { name: "New link" }));
    await waitFor(() => expect(screen.getByTestId("probe").textContent).toBe("new=1"));
  });
});
