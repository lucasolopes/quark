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
      aggregates: { total: 42, first_ts: 1700000000, last_ts: 1700100000, per_day: { "2024-01-01": 42 }, per_country: { BR: 40, US: 2 }, per_device: { Mobile: 30, Desktop: 12 } },
      recent: [],
    }), { status: 200 }));
    render(wrap("6lB362J"));
    expect(await screen.findByText("42")).toBeInTheDocument();
  });

  it("empty state when total is 0", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({
      aggregates: { total: 0, first_ts: 0, last_ts: 0, per_day: {}, per_country: {}, per_device: {} },
      recent: [],
    }), { status: 200 }));
    render(wrap("6lB362J"));
    expect(await screen.findByText(/no clicks yet/i)).toBeInTheDocument();
  });
});
