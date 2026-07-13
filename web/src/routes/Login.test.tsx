import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Login } from "./Login";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}><MemoryRouter>{ui}</MemoryRouter></QueryClientProvider>;
}

describe("Login", () => {
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

  it("token válido guarda e a sonda é chamada", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ links: [], next_after: null }), { status: 200 }),
    );
    render(wrap(<Login />));
    await userEvent.type(screen.getByLabelText(/token/i), "segredo");
    await userEvent.click(screen.getByRole("button", { name: /entrar/i }));
    expect(localStorage.getItem("quark_admin_token")).toBe("segredo");
  });

  it("token inválido mostra erro", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 401 }));
    render(wrap(<Login />));
    await userEvent.type(screen.getByLabelText(/token/i), "errado");
    await userEvent.click(screen.getByRole("button", { name: /entrar/i }));
    expect(await screen.findByText(/token inválido/i)).toBeInTheDocument();
  });
});
