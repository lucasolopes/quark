import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { LinkStats } from "./LinkStats";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { withProviders } from "@/test-utils";

function wrap(code: string) {
  return withProviders(
    <MemoryRouter initialEntries={[`/links/${code}`]}>
      <Routes>
        <Route path="/links/:code" element={<LinkStats />} />
      </Routes>
    </MemoryRouter>,
    { withRouter: false },
  );
}

describe("LinkStats", () => {
  beforeEach(() => { localStorage.setItem("quark_admin_token", "s"); vi.restoreAllMocks(); });

  it("shows the total clicks", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({
      aggregates: {
        total: 42, first_ts: 1700000000, last_ts: 1700100000,
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
    }), { status: 200 }));
    render(wrap("6lB362J"));
    expect(await screen.findByText("42")).toBeInTheDocument();
  });

  it("shows the bots-excluded count as a separate stat card", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({
      aggregates: {
        total: 42, first_ts: 1700000000, last_ts: 1700100000,
        bots: 7,
        per_day: { "2024-01-01": 42 },
        per_country: { BR: 40, US: 2 },
        per_device: { Mobile: 30, Desktop: 12 },
        per_os: { Windows: 20, iOS: 22 },
        per_browser: { Chrome: 25, Safari: 17 },
        per_referer: { "news.ycombinator.com": 30, direct: 12 },
        per_city: {},
      },
      recent: [],
    }), { status: 200 }));
    render(wrap("6lB362J"));
    expect(await screen.findByText("Bots (excluded)")).toBeInTheDocument();
    expect(await screen.findByText("7")).toBeInTheDocument();
  });

  it("empty state when total is 0", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({
      aggregates: {
        total: 0, first_ts: 0, last_ts: 0,
        bots: 0,
        per_day: {}, per_country: {}, per_device: {},
        per_os: {}, per_browser: {}, per_referer: {}, per_city: {},
        per_variant: {},
      },
      recent: [],
    }), { status: 200 }));
    render(wrap("6lB362J"));
    expect(await screen.findByText(/no clicks yet/i)).toBeInTheDocument();
  });

  it("shows the new OS, browser and referrer charts, and hides the city chart when per_city is empty", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({
      aggregates: {
        total: 42, first_ts: 1700000000, last_ts: 1700100000,
        bots: 0,
        per_day: { "2024-01-01": 42 },
        per_country: { BR: 40, US: 2 },
        per_device: { Mobile: 30, Desktop: 12 },
        per_os: { Windows: 20, iOS: 22 },
        per_browser: { Chrome: 25, Safari: 17 },
        per_referer: { "news.ycombinator.com": 30, direct: 12 },
        per_city: {},
      },
      recent: [],
    }), { status: 200 }));
    render(wrap("6lB362J"));
    expect(await screen.findByText("Clicks per OS")).toBeInTheDocument();
    expect(screen.getByText("Clicks per browser")).toBeInTheDocument();
    expect(screen.getByText("Clicks per referrer")).toBeInTheDocument();
    expect(screen.queryByText("Clicks per city")).not.toBeInTheDocument();
  });

  it("shows the city chart when per_city has data", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({
      aggregates: {
        total: 5, first_ts: 1700000000, last_ts: 1700100000,
        bots: 1,
        per_day: { "2024-01-01": 5 },
        per_country: { BR: 5 },
        per_device: { Mobile: 5 },
        per_os: { iOS: 5 },
        per_browser: { Safari: 5 },
        per_referer: { direct: 5 },
        per_city: { "Sao Paulo": 5 },
      },
      recent: [],
    }), { status: 200 }));
    render(wrap("6lB362J"));
    expect(await screen.findByText("Clicks per city")).toBeInTheDocument();
  });

  it("keeps its own back-to-links button even when the stats fetch errors (LUC-61)", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("boom", { status: 500 }));
    render(wrap("6lB362J"));
    expect(await screen.findByText(/could not load stats/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /back to links/i })).toBeInTheDocument();
  });
});
