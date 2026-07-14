import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Links } from "./Links";
import { withProviders } from "@/test-utils";

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status });
}

/** Mocks `fetch`, responding based on which URL was called (search `q=` vs base list). */
function mockFetchByUrl(handler: (url: string) => Response) {
  vi.spyOn(globalThis, "fetch").mockImplementation((input) => Promise.resolve(handler(String(input))));
}

describe("Links", () => {
  beforeEach(() => { localStorage.setItem("quark_admin_token", "s"); vi.restoreAllMocks(); });

  it("renders the loaded links", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({
      links: [{ id: 1, code: "6lB362J", url: "https://example.com/a", alias: "promo", expiry: null, created: 1700000000 }],
      next_after: null,
    }), { status: 200 }));
    render(withProviders(<Links />));
    expect(await screen.findByText("6lB362J")).toBeInTheDocument();
    expect(screen.getByText(/example\.com\/a/)).toBeInTheDocument();
  });

  it("searches on the server when the backend supports it (?q= in the querystring)", async () => {
    const base = {
      links: [
        { id: 1, code: "AAA0000", url: "https://cat.com", expiry: null, created: 1 },
        { id: 2, code: "BBB1111", url: "https://dog.com", expiry: null, created: 2 },
      ],
      next_after: null,
    };
    mockFetchByUrl((url) =>
      url.includes("q=")
        ? jsonResponse({ links: base.links.filter((l) => l.url.includes("dog")), next_after: null })
        : jsonResponse(base),
    );
    render(withProviders(<Links />));
    await screen.findByText("AAA0000");
    await userEvent.type(screen.getByRole("searchbox"), "dog");
    await waitFor(() => {
      expect(screen.getByText("BBB1111")).toBeInTheDocument();
      expect(screen.queryByText("AAA0000")).not.toBeInTheDocument();
    });
  });

  it("falls back to client-side filtering when the search returns 501", async () => {
    const base = {
      links: [
        { id: 1, code: "AAA0000", url: "https://github.com/x", expiry: null, created: 1 },
        { id: 2, code: "BBB1111", url: "https://example.com", expiry: null, created: 2 },
      ],
      next_after: null,
    };
    mockFetchByUrl((url) => (url.includes("q=") ? new Response("{}", { status: 501 }) : jsonResponse(base)));
    render(withProviders(<Links />));
    await screen.findByText("AAA0000");
    await userEvent.type(screen.getByRole("searchbox"), "github");
    await waitFor(() => {
      expect(screen.getByText("AAA0000")).toBeInTheDocument();
      expect(screen.queryByText("BBB1111")).not.toBeInTheDocument();
    });
  });

  it("search empty state shows the message with the term", async () => {
    const base = { links: [{ id: 1, code: "AAA0000", url: "https://cat.com", expiry: null, created: 1 }], next_after: null };
    mockFetchByUrl((url) =>
      url.includes("q=") ? jsonResponse({ links: [], next_after: null }) : jsonResponse(base),
    );
    render(withProviders(<Links />));
    await screen.findByText("AAA0000");
    await userEvent.type(screen.getByRole("searchbox"), "zzz");
    expect(await screen.findByText(/no links found for "zzz"/i)).toBeInTheDocument();
  });

  it("search error (non-501, 500) shows the error state, not the 'no results' one", async () => {
    const base = { links: [{ id: 1, code: "AAA0000", url: "https://cat.com", expiry: null, created: 1 }], next_after: null };
    mockFetchByUrl((url) => (url.includes("q=") ? new Response("{}", { status: 500 }) : jsonResponse(base)));
    render(withProviders(<Links />));
    await screen.findByText("AAA0000");
    await userEvent.type(screen.getByRole("searchbox"), "zzz");
    expect(await screen.findByText(/could not search/i)).toBeInTheDocument();
    expect(screen.queryByText(/no links found for "zzz"/i)).not.toBeInTheDocument();
  });

  it("empty state", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({ links: [], next_after: null }), { status: 200 }));
    render(withProviders(<Links />));
    expect(await screen.findByText(/no links yet/i)).toBeInTheDocument();
  });
});
