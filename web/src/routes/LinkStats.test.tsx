import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { LinkStats } from "./LinkStats";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

function wrap(code: string) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return (
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[`/links/${code}`]}>
        <Routes><Route path="/links/:code" element={<LinkStats />} /></Routes>
      </MemoryRouter>
    </QueryClientProvider>
  );
}

describe("LinkStats", () => {
  beforeEach(() => { localStorage.setItem("quark_admin_token", "s"); vi.restoreAllMocks(); });

  it("mostra o total de cliques", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({
      aggregates: { total: 42, first_ts: 1700000000, last_ts: 1700100000, per_day: { "2024-01-01": 42 }, per_country: { BR: 40, US: 2 }, per_device: { Mobile: 30, Desktop: 12 } },
      recent: [],
    }), { status: 200 }));
    render(wrap("6lB362J"));
    expect(await screen.findByText("42")).toBeInTheDocument();
  });

  it("estado vazio quando total 0", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({
      aggregates: { total: 0, first_ts: 0, last_ts: 0, per_day: {}, per_country: {}, per_device: {} },
      recent: [],
    }), { status: 200 }));
    render(wrap("6lB362J"));
    expect(await screen.findByText(/sem cliques ainda/i)).toBeInTheDocument();
  });
});
