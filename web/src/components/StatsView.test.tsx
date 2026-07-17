import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
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

  it("shows the heading, subtitle and total clicks on success", async () => {
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
    expect(await screen.findByText("42")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Stats" })).toBeInTheDocument();
    expect(screen.getByText(/6lB362J/)).toBeInTheDocument();
  });
});
