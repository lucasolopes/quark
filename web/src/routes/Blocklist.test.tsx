import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { Blocklist } from "./Blocklist";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return <QueryClientProvider client={qc}>{ui}</QueryClientProvider>;
}

describe("Blocklist", () => {
  beforeEach(() => { localStorage.setItem("quark_admin_token", "s"); vi.restoreAllMocks(); });

  it("lista os domínios", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({ domains: ["evil.com"] }), { status: 200 }));
    render(wrap(<Blocklist />));
    expect(await screen.findByText("evil.com")).toBeInTheDocument();
  });

  it("estado vazio", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({ domains: [] }), { status: 200 }));
    render(wrap(<Blocklist />));
    expect(await screen.findByText(/nenhum domínio bloqueado/i)).toBeInTheDocument();
  });
});
