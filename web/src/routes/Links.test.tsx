import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Links } from "./Links";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}><MemoryRouter>{ui}</MemoryRouter></QueryClientProvider>;
}

describe("Links", () => {
  beforeEach(() => { localStorage.setItem("quark_admin_token", "s"); vi.restoreAllMocks(); });

  it("renderiza os links carregados", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({
      links: [{ id: 1, code: "6lB362J", url: "https://exemplo.com/a", alias: "promo", expiry: null, created: 1700000000 }],
      next_after: null,
    }), { status: 200 }));
    render(wrap(<Links />));
    expect(await screen.findByText("6lB362J")).toBeInTheDocument();
    expect(screen.getByText(/exemplo\.com\/a/)).toBeInTheDocument();
  });

  it("busca filtra a lista carregada", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({
      links: [
        { id: 1, code: "AAA0000", url: "https://gato.com", expiry: null, created: 1 },
        { id: 2, code: "BBB1111", url: "https://cachorro.com", expiry: null, created: 2 },
      ],
      next_after: null,
    }), { status: 200 }));
    render(wrap(<Links />));
    await screen.findByText("AAA0000");
    await userEvent.type(screen.getByRole("searchbox"), "cachorro");
    expect(screen.queryByText("AAA0000")).not.toBeInTheDocument();
    expect(screen.getByText("BBB1111")).toBeInTheDocument();
  });

  it("estado vazio", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({ links: [], next_after: null }), { status: 200 }));
    render(wrap(<Links />));
    expect(await screen.findByText(/nenhum link ainda/i)).toBeInTheDocument();
  });
});
