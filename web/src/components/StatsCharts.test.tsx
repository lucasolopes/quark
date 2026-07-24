import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { StatsCharts } from "./StatsCharts";
import { withProviders } from "@/test-utils";
import type { Aggregates } from "@/lib/types";

const baseAggregates: Aggregates = {
  total: 10,
  first_ts: 1700000000,
  last_ts: 1700100000,
  bots: 0,
  per_day: { "2024-01-01": 10 },
  per_country: { BR: 8, US: 2 },
  per_device: { Mobile: 6, Desktop: 4 },
  per_os: {},
  per_browser: {},
  per_referer: {},
  per_city: {},
  per_variant: {},
};

describe("StatsCharts — per-variant chart", () => {
  it("renders the per-variant chart when per_variant has data", () => {
    render(
      withProviders(
        <StatsCharts aggregates={{ ...baseAggregates, per_variant: { "0": 7, "1": 3 } }} />,
        { withRouter: false },
      ),
    );
    expect(screen.getByText("Clicks per variant")).toBeInTheDocument();
  });

  it("does not render the per-variant chart when per_variant is empty", () => {
    render(withProviders(<StatsCharts aggregates={baseAggregates} />, { withRouter: false }));
    expect(screen.queryByText("Clicks per variant")).not.toBeInTheDocument();
  });
});

describe("StatsCharts — country/device/browser moved to MeterBar (v2)", () => {
  it("does not render the country, device or browser breakdowns even when their data is present", () => {
    render(
      withProviders(
        <StatsCharts aggregates={{ ...baseAggregates, per_browser: { Chrome: 6, Safari: 4 } }} />,
        { withRouter: false },
      ),
    );
    // `StatsView` renders these three as MeterBar rows in its own distribution
    // cards, not as recharts here — this component keeps day/OS/referrer/city/variant only.
    expect(screen.queryByText("Clicks per country")).not.toBeInTheDocument();
    expect(screen.queryByText("Clicks per device")).not.toBeInTheDocument();
    expect(screen.queryByText("Clicks per browser")).not.toBeInTheDocument();
  });

  it("still renders the per-day chart title (now a lime-gradient bar chart)", () => {
    render(withProviders(<StatsCharts aggregates={baseAggregates} />, { withRouter: false }));
    expect(screen.getByText("Clicks per day")).toBeInTheDocument();
  });
});
