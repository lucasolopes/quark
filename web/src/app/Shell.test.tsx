import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Route, Routes, useLocation, useSearchParams } from "react-router-dom";
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

/** Shows the current route's pathname, so a drawer nav-item click is observable. */
function LocationProbe() {
  const location = useLocation();
  return <div data-testid="location-probe">{location.pathname}</div>;
}

function renderShellWithLocationProbe(initialEntries: string[]) {
  return render(
    withProviders(
      <Routes>
        <Route path="/" element={<Shell />}>
          <Route path="links" element={<LocationProbe />} />
          <Route path="analytics" element={<LocationProbe />} />
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

describe("Shell v2 — mobile drawer + expandable search", () => {
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

  it("shows a hamburger button that opens the mobile nav drawer", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(meResponse({ authenticated: true, oidc_enabled: false }));
    render(withProviders(<Shell />, { initialEntries: ["/links"] }));
    await waitFor(() => expect(screen.getByText("quark")).toBeInTheDocument());
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    const hamburger = screen.getByRole("button", { name: "Open menu" });
    // Regression guard (LUC-96 review finding): mobile primary control must
    // meet the 44px touch target, not the base `size-icon` 32px.
    expect(hamburger.className).toMatch(/\bsize-11\b/);

    await userEvent.click(hamburger);
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });

  it("clicking a nav item inside the drawer navigates there and closes the drawer", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(meResponse({ authenticated: true, oidc_enabled: false }));
    renderShellWithLocationProbe(["/links"]);
    await waitFor(() => expect(screen.getByText("quark")).toBeInTheDocument());

    await userEvent.click(screen.getByRole("button", { name: "Open menu" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("link", { name: "Analytics" }));

    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(screen.getByTestId("location-probe")).toHaveTextContent("/analytics");
  });

  it("the lupa expands the mobile search row with an autofocused input, and the X collapses it again", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(meResponse({ authenticated: true, oidc_enabled: false }));
    render(withProviders(<Shell />, { initialEntries: ["/links"] }));
    await waitFor(() => expect(screen.getByText("quark")).toBeInTheDocument());
    expect(screen.queryByTestId("mobile-search-input")).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Search" }));
    const mobileInput = await screen.findByTestId("mobile-search-input");
    expect(mobileInput).toHaveFocus();

    // The close button is the X inside the search row; use getAllByRole to get the specific instance
    const closeButtons = screen.getAllByRole("button", { name: "Close search" });
    // The X button is the one in the search row (the second instance after lupa)
    await userEvent.click(closeButtons[closeButtons.length - 1]);
    expect(screen.queryByTestId("mobile-search-input")).not.toBeInTheDocument();
  });

  it("mobile topbar order: lupa sits adjacent to hamburger on the LEFT (not with New-link on right)", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(meResponse({ authenticated: true, oidc_enabled: false }));
    render(withProviders(<Shell />, { initialEntries: ["/links"] }));
    await waitFor(() => expect(screen.getByText("quark")).toBeInTheDocument());

    const hamburger = screen.getByRole("button", { name: "Open menu" });
    const lupa = screen.getByRole("button", { name: "Search" });
    const newLink = screen.getByRole("button", { name: "New link" });

    // Hamburger and lupa must share the same parent (left wrapper).
    expect(hamburger.parentElement).toBe(lupa.parentElement);

    // New link must NOT be in that same parent (it's in the right cluster).
    expect(newLink.parentElement).not.toBe(hamburger.parentElement);
  });

  it("lupa aria-label reflects its expanded state: 'Search' when closed, 'Close search' when open", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(meResponse({ authenticated: true, oidc_enabled: false }));
    render(withProviders(<Shell />, { initialEntries: ["/links"] }));
    await waitFor(() => expect(screen.getByText("quark")).toBeInTheDocument());

    const lupa = screen.getByRole("button", { name: "Search" });
    expect(lupa).toHaveAttribute("aria-label", "Search");

    await userEvent.click(lupa);
    await waitFor(() => expect(lupa).toHaveAttribute("aria-label", "Close search"));

    await userEvent.click(lupa);
    await waitFor(() => expect(lupa).toHaveAttribute("aria-label", "Search"));
  });
});
