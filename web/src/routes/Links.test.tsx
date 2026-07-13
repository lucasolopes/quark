import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Links } from "./Links";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}><MemoryRouter>{ui}</MemoryRouter></QueryClientProvider>;
}

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status });
}

/** Mocka `fetch` respondendo com base em qual URL foi chamada (busca `q=` vs lista base). */
function mockFetchByUrl(handler: (url: string) => Response) {
  vi.spyOn(globalThis, "fetch").mockImplementation((input) => Promise.resolve(handler(String(input))));
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

  it("busca no servidor quando o backend suporta (?q= na querystring)", async () => {
    const base = {
      links: [
        { id: 1, code: "AAA0000", url: "https://gato.com", expiry: null, created: 1 },
        { id: 2, code: "BBB1111", url: "https://cachorro.com", expiry: null, created: 2 },
      ],
      next_after: null,
    };
    mockFetchByUrl((url) =>
      url.includes("q=")
        ? jsonResponse({ links: base.links.filter((l) => l.url.includes("cachorro")), next_after: null })
        : jsonResponse(base),
    );
    render(wrap(<Links />));
    await screen.findByText("AAA0000");
    await userEvent.type(screen.getByRole("searchbox"), "cachorro");
    // debounce (~300ms) + fetch da busca server-side; as duas condições juntas
    // evitam um falso-positivo no meio do caminho (tabela vazia por estar
    // carregando ainda conta como "AAA0000 ausente").
    await waitFor(() => {
      expect(screen.getByText("BBB1111")).toBeInTheDocument();
      expect(screen.queryByText("AAA0000")).not.toBeInTheDocument();
    });
  });

  it("cai pro filtro client-side quando a busca retorna 501", async () => {
    const base = {
      links: [
        { id: 1, code: "AAA0000", url: "https://github.com/x", expiry: null, created: 1 },
        { id: 2, code: "BBB1111", url: "https://example.com", expiry: null, created: 2 },
      ],
      next_after: null,
    };
    mockFetchByUrl((url) => (url.includes("q=") ? new Response("{}", { status: 501 }) : jsonResponse(base)));
    render(wrap(<Links />));
    await screen.findByText("AAA0000");
    await userEvent.type(screen.getByRole("searchbox"), "github");
    // após debounce + 501, o painel cai pro filtro client-side sobre a lista
    // base já carregada; as duas condições juntas evitam um falso-positivo
    // no meio do caminho (tabela vazia por estar carregando ainda).
    await waitFor(() => {
      expect(screen.getByText("AAA0000")).toBeInTheDocument();
      expect(screen.queryByText("BBB1111")).not.toBeInTheDocument();
    });
  });

  it("estado vazio de busca mostra a mensagem com o termo", async () => {
    const base = { links: [{ id: 1, code: "AAA0000", url: "https://gato.com", expiry: null, created: 1 }], next_after: null };
    mockFetchByUrl((url) =>
      url.includes("q=") ? jsonResponse({ links: [], next_after: null }) : jsonResponse(base),
    );
    render(wrap(<Links />));
    await screen.findByText("AAA0000");
    await userEvent.type(screen.getByRole("searchbox"), "zzz");
    expect(await screen.findByText(/nenhum link encontrado para "zzz"/i)).toBeInTheDocument();
  });

  it("busca com erro não-501 (500) mostra estado de erro, não o de 'nenhum resultado'", async () => {
    const base = { links: [{ id: 1, code: "AAA0000", url: "https://gato.com", expiry: null, created: 1 }], next_after: null };
    mockFetchByUrl((url) => (url.includes("q=") ? new Response("{}", { status: 500 }) : jsonResponse(base)));
    render(wrap(<Links />));
    await screen.findByText("AAA0000");
    await userEvent.type(screen.getByRole("searchbox"), "zzz");
    expect(await screen.findByText(/não foi possível buscar/i)).toBeInTheDocument();
    expect(screen.queryByText(/nenhum link encontrado para "zzz"/i)).not.toBeInTheDocument();
  });

  it("estado vazio", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({ links: [], next_after: null }), { status: 200 }));
    render(wrap(<Links />));
    expect(await screen.findByText(/nenhum link ainda/i)).toBeInTheDocument();
  });
});
