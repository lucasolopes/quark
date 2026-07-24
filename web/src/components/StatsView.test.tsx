import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import { StatsView } from "./StatsView";
import { withProviders } from "@/test-utils";

function wrap(code: string) {
  return withProviders(<StatsView code={code} />);
}

describe("StatsView", () => {
  beforeEach(() => {
    localStorage.setItem("quark_admin_token", "s");
    vi.restoreAllMocks();
  });

  it("shows the skeleton while pending", () => {
    vi.spyOn(globalThis, "fetch").mockReturnValue(new Promise(() => {}));
    const { container } = render(wrap("6lB362J"));
    expect(container.querySelectorAll('[data-slot="skeleton"]').length).toBeGreaterThan(0);
  });

  it("shows the KPI stat cards (top country/device) and the country/device/browser distributions on success", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(
        JSON.stringify({
          aggregates: {
            total: 42,
            first_ts: 1700000000,
            last_ts: 1700100000,
            bots: 7,
            per_day: { "2024-01-01": 42 },
            per_country: { BR: 40, US: 2 },
            per_device: { Mobile: 30, Desktop: 12 },
            per_os: { Windows: 20, iOS: 22 },
            per_browser: { Chrome: 25, Safari: 17 },
            per_referer: { "news.ycombinator.com": 30, direct: 12 },
            per_city: {},
            per_variant: {},
          },
          recent: [],
        }),
        { status: 200 },
      ),
    );
    render(wrap("6lB362J"));
    // KPI row (v2: StatCard) — total / top country / top device / bots.
    expect(await screen.findByText("42")).toBeInTheDocument();
    expect(screen.getByText("Total clicks")).toBeInTheDocument();
    expect(screen.getByText("Bots (excluded)")).toBeInTheDocument();
    expect(screen.getByText("7")).toBeInTheDocument();
    // Top country/device cards are derived from per_country/per_device
    // (already fetched), each as a share of the TRUE total (review fix).
    const topCountryCard = screen.getByText("Top country").parentElement as HTMLElement;
    expect(within(topCountryCard).getByText("BR")).toBeInTheDocument();
    expect(within(topCountryCard).getByText("95%")).toBeInTheDocument();
    const topDeviceCard = screen.getByText("Top device").parentElement as HTMLElement;
    expect(within(topDeviceCard).getByText("Mobile")).toBeInTheDocument();
    expect(within(topDeviceCard).getByText("71%")).toBeInTheDocument();
    // First/last click move out of the KPI grid into a muted meta line below
    // it — the metrics stay visible, just no longer their own StatCards.
    expect(screen.getByText(/First click/)).toBeInTheDocument();
    expect(screen.getByText(/Last click/)).toBeInTheDocument();
    // StatsView itself carries no page-identity heading anymore (LUC-61's
    // "Stats" h1 moved to `LinkStats`'s `PageHeader`, since `StatsView` is
    // also embedded standalone in `Analytics` for whichever link is selected).
    expect(screen.queryByRole("heading", { name: "Stats" })).not.toBeInTheDocument();
    // Country/device/browser distributions (v2: MeterBar rows, % mono at the right).
    const countryCard = screen.getByText("Clicks per country").closest('[data-slot="card"]') as HTMLElement;
    expect(within(countryCard).getByText("BR")).toBeInTheDocument();
    expect(within(countryCard).getByText("95%")).toBeInTheDocument();
    expect(within(countryCard).getByText("US")).toBeInTheDocument();
    const deviceCard = screen.getByText("Clicks per device").closest('[data-slot="card"]') as HTMLElement;
    expect(within(deviceCard).getByText("Mobile")).toBeInTheDocument();
    expect(within(deviceCard).getByText("Desktop")).toBeInTheDocument();
    expect(within(deviceCard).getByText("Chrome")).toBeInTheDocument();
  });

  it("computes the country pct (KPI + MeterBar) over the FULL total, not the rendered top-8 slice (review fix)", async () => {
    // 10 countries, distinct counts so every rendered row's pct is unique.
    // BR=20 is the highest and lands inside the rendered top-8 slice, which
    // sums to 104 (excludes IN=4 and MX=2, the two smallest). Dividing by
    // that slice would wrongly read 20/104 ≈ 19.2% -> "19%"; the fix divides
    // by the full 10-country total (110), so BR's true share is
    // 20/110 ≈ 18.2% -> "18%".
    const perCountry = { BR: 20, US: 18, GB: 16, DE: 14, FR: 12, JP: 10, CA: 8, AU: 6, IN: 4, MX: 2 };
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(
        JSON.stringify({
          aggregates: {
            total: 110,
            first_ts: 1700000000,
            last_ts: 1700100000,
            bots: 0,
            per_day: {},
            per_country: perCountry,
            per_device: {},
            per_os: {},
            per_browser: {},
            per_referer: {},
            per_city: {},
            per_variant: {},
          },
          recent: [],
        }),
        { status: 200 },
      ),
    );
    render(wrap("6lB362J"));
    const countryCard = (await screen.findByText("Clicks per country")).closest('[data-slot="card"]') as HTMLElement;
    expect(within(countryCard).getByText("18%")).toBeInTheDocument();
    expect(within(countryCard).queryByText("19%")).not.toBeInTheDocument();
    // Same underlying total feeds the top-country KPI card.
    const topCountryCard = screen.getByText("Top country").parentElement as HTMLElement;
    expect(within(topCountryCard).getByText("BR")).toBeInTheDocument();
    expect(within(topCountryCard).getByText("18%")).toBeInTheDocument();
  });

  it("shows an em dash on the top country/device KPI cards when their breakdown maps are empty", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(
        JSON.stringify({
          aggregates: {
            total: 5,
            first_ts: 1700000000,
            last_ts: 1700100000,
            bots: 0,
            per_day: { "2024-01-01": 5 },
            per_country: {},
            per_device: {},
            per_os: {},
            per_browser: {},
            per_referer: {},
            per_city: {},
            per_variant: {},
          },
          recent: [],
        }),
        { status: 200 },
      ),
    );
    render(wrap("6lB362J"));
    expect(await screen.findByText("Total clicks")).toBeInTheDocument();
    const topCountryCard = screen.getByText("Top country").parentElement as HTMLElement;
    expect(within(topCountryCard).getByText("—")).toBeInTheDocument();
    const topDeviceCard = screen.getByText("Top device").parentElement as HTMLElement;
    expect(within(topDeviceCard).getByText("—")).toBeInTheDocument();
  });

  it("error state shows a neutral message and retry, with no navigation link (LUC-61)", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("boom", { status: 500 }));
    render(wrap("6lB362J"));
    expect(await screen.findByText("Could not load stats.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /try again/i })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: /back to links/i })).not.toBeInTheDocument();
    expect(screen.queryByText(/back to links/i)).not.toBeInTheDocument();
  });
});
