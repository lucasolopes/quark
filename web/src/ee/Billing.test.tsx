import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useSearchParams } from "react-router-dom";
import { Billing } from "./Billing";
import { withProviders } from "@/test-utils";
import { Toaster } from "@/components/ui/sonner";
import type { BillingCatalog, MeResponse } from "@/lib/types";

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status });
}

const CATALOG: BillingCatalog = {
  current_plan: "free",
  currency_locked: null,
  prices_available: true,
  plans: [
    {
      plan: "free",
      limits: { domains: 3, members: 1, automation_per_month: 100, tracked_clicks_per_month: 50000, retention_days: 30 },
      features: [],
      prices: null,
    },
    {
      plan: "starter",
      limits: { domains: 10, members: 3, automation_per_month: 5000, tracked_clicks_per_month: 250000, retention_days: 365 },
      features: ["webhooks", "integrations"],
      prices: { monthly: { usd_cents: 400, brl_cents: 1900 }, yearly: { usd_cents: 4000, brl_cents: 19000 } },
    },
    {
      plan: "pro",
      limits: { domains: 50, members: 10, automation_per_month: 50000, tracked_clicks_per_month: 1000000, retention_days: 730 },
      features: ["webhooks", "integrations"],
      prices: { monthly: { usd_cents: 1400, brl_cents: 5900 }, yearly: { usd_cents: 14000, brl_cents: 59000 } },
    },
    {
      plan: "business",
      limits: { domains: null, members: null, automation_per_month: 500000, tracked_clicks_per_month: 5000000, retention_days: 1095 },
      features: ["webhooks", "integrations", "sso"],
      prices: { monthly: { usd_cents: 3900, brl_cents: 14900 }, yearly: { usd_cents: 39000, brl_cents: 149000 } },
    },
    {
      plan: "custom",
      limits: { domains: null, members: null, automation_per_month: null, tracked_clicks_per_month: null, retention_days: null },
      features: ["webhooks", "integrations", "sso"],
      prices: null,
    },
  ],
};

const ME_OWNER: MeResponse = {
  authenticated: true,
  oidc_enabled: false,
  multi_tenant: true,
  current_tenant: 1,
  memberships: [{ tenant_id: 1, name: "Acme", slug: "acme", role: "owner" }],
};

const ME_MEMBER: MeResponse = {
  authenticated: true,
  oidc_enabled: false,
  multi_tenant: true,
  current_tenant: 1,
  memberships: [{ tenant_id: 1, name: "Acme", slug: "acme", role: "member" }],
};

/** Routes a mocked `fetch` call to the right canned response by URL + method, mirroring `Members.test.tsx`'s pattern. */
function mockFetch(opts: {
  me?: MeResponse;
  catalog?: BillingCatalog | { prices_available: false } & Partial<BillingCatalog>;
  checkout?: () => Response;
  portal?: () => Response;
}) {
  const { me = ME_OWNER, catalog = CATALOG, checkout, portal } = opts;
  return vi.spyOn(globalThis, "fetch").mockImplementation((input, init) => {
    const url = String(input);
    if (url.includes("/admin/billing/checkout") && init?.method === "POST") {
      return Promise.resolve(checkout ? checkout() : jsonResponse({ url: "https://checkout.stripe.com/session" }));
    }
    if (url.includes("/admin/billing/portal") && init?.method === "POST") {
      return Promise.resolve(portal ? portal() : jsonResponse({ url: "https://billing.stripe.com/portal" }));
    }
    if (url.includes("/admin/billing/catalog")) {
      return Promise.resolve(jsonResponse(catalog));
    }
    if (url.includes("/admin/me")) {
      return Promise.resolve(jsonResponse(me));
    }
    return Promise.resolve(jsonResponse({}));
  });
}

/** Test-only sibling of `Billing`, sharing its `MemoryRouter` context: lets a test move the
 * `?highlight=` param without unmounting `Billing` (mirroring `App.tsx`'s in-place navigation
 * from a second plan-limit toast while the user is already on this screen). */
function HighlightSwitcher({ to }: { to: string }) {
  const [, setSearchParams] = useSearchParams();
  return (
    <button type="button" onClick={() => setSearchParams({ highlight: to })}>
      switch highlight
    </button>
  );
}

describe("Billing", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.restoreAllMocks();
  });

  it("renders the five plan cards and marks the current one", async () => {
    mockFetch({ me: ME_OWNER, catalog: CATALOG });
    render(withProviders(<Billing />));

    expect(await screen.findByText("Free")).toBeInTheDocument();
    expect(screen.getByText("Starter")).toBeInTheDocument();
    expect(screen.getByText("Pro")).toBeInTheDocument();
    expect(screen.getByText("Business")).toBeInTheDocument();
    expect(screen.getByText("Custom")).toBeInTheDocument();

    // The catalog's `current_plan` is "free": that card carries the "current plan" badge.
    expect(screen.getByText("Current plan")).toBeInTheDocument();

    // Default cycle is monthly, default currency is USD: the starter card shows $4.
    expect(screen.getByText(/\$4\b/)).toBeInTheDocument();

    // Switching the currency toggle to BRL re-renders the same price in reais.
    await userEvent.click(screen.getByRole("button", { name: "BRL" }));
    expect(await screen.findByText(/R\$\s?19\b/)).toBeInTheDocument();
  });

  it("disables the upgrade button for a non-owner with a tooltip", async () => {
    mockFetch({ me: ME_MEMBER, catalog: CATALOG });
    render(withProviders(<Billing />));

    await screen.findByText("Starter");
    const upgradeButtons = screen.getAllByRole("button", { name: /upgrade/i });
    expect(upgradeButtons.length).toBeGreaterThan(0);
    for (const button of upgradeButtons) {
      expect(button).toBeDisabled();
      expect(button).toHaveAttribute("title", "Only the workspace owner can change the plan.");
    }
  });

  it("switches to the portal path on 409", async () => {
    const fetchMock = mockFetch({
      me: ME_OWNER,
      catalog: CATALOG,
      checkout: () => jsonResponse({ error: "subscription_active" }, 409),
    });
    const assignMock = vi.fn();
    vi.spyOn(window, "location", "get").mockReturnValue({ ...window.location, assign: assignMock } as Location);

    render(withProviders(<Billing />));
    await screen.findByText("Starter");

    const upgradeButtons = screen.getAllByRole("button", { name: /upgrade/i });
    await userEvent.click(upgradeButtons[0]);

    await waitFor(() => {
      const portalCall = fetchMock.mock.calls.find(([u]) => String(u).includes("/admin/billing/portal"));
      expect(portalCall).toBeDefined();
    });
    await waitFor(() => {
      expect(assignMock).toHaveBeenCalledWith("https://billing.stripe.com/portal");
    });
  });

  it("hides purchase buttons when prices are unavailable", async () => {
    mockFetch({
      me: ME_OWNER,
      catalog: { ...CATALOG, prices_available: false },
    });
    render(withProviders(<Billing />));

    await screen.findByText("Starter");
    expect(screen.queryByRole("button", { name: /upgrade/i })).not.toBeInTheDocument();
    expect(screen.getByText(/billing isn't configured/i)).toBeInTheDocument();
  });

  it("shows the success toast when returning from checkout, and clears the polling interval on unmount", async () => {
    mockFetch({ me: ME_OWNER, catalog: CATALOG });
    // sonner's `Toaster` reads `prefers-color-scheme` via `matchMedia`, which jsdom doesn't implement.
    vi.stubGlobal("matchMedia", (query: string) => ({
      matches: false,
      media: query,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    }));
    const clearIntervalSpy = vi.spyOn(globalThis, "clearInterval");

    const { unmount } = render(
      withProviders(
        <>
          <Toaster />
          <Billing />
        </>,
        { initialEntries: ["/settings/billing?checkout=success"] },
      ),
    );

    expect(await screen.findByText(/payment received/i)).toBeInTheDocument();

    // The `?checkout=success` effect schedules a 3-attempt polling interval to catch the
    // webhook-delayed plan change; unmounting before it finishes must clear it (not leave it
    // firing `catalogQuery.refetch()` against a torn-down component).
    unmount();
    expect(clearIntervalSpy).toHaveBeenCalled();
  });

  it("scrolls again when the highlighted plan changes while the screen stays mounted", async () => {
    mockFetch({ me: ME_OWNER, catalog: CATALOG });
    const scrollIntoViewMock = vi.fn();
    Element.prototype.scrollIntoView = scrollIntoViewMock;

    render(
      withProviders(
        <>
          <HighlightSwitcher to="pro" />
          <Billing />
        </>,
        { initialEntries: ["/settings/billing?highlight=starter"] },
      ),
    );

    await screen.findByText("Starter");
    await waitFor(() => expect(scrollIntoViewMock).toHaveBeenCalledTimes(1));

    // Same component instance, no unmount: a second `?highlight=` (e.g. a follow-up 402 toast
    // sending the user to a different plan) must scroll again, not stay silently parked on the
    // first card.
    await userEvent.click(screen.getByRole("button", { name: /switch highlight/i }));
    await waitFor(() => expect(scrollIntoViewMock).toHaveBeenCalledTimes(2));
  });

  it("shows Manage in portal immediately for an owner whose current plan is already paid", async () => {
    mockFetch({ me: ME_OWNER, catalog: { ...CATALOG, current_plan: "starter" } });
    render(withProviders(<Billing />));

    await screen.findByText("Starter");
    // `current_plan` is a paid plan on mount: the screen assumes an active subscription right
    // away instead of waiting for a 409 round-trip to discover it.
    const portalButtons = screen.getAllByRole("button", { name: /manage in portal/i });
    expect(portalButtons.length).toBeGreaterThan(0);
    expect(screen.queryByRole("button", { name: /^upgrade$/i })).not.toBeInTheDocument();
  });
});
