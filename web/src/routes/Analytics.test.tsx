import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Analytics } from "./Analytics";
import { withProviders } from "@/test-utils";

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status });
}

/** Mocks `fetch`, responding based on which URL was called (search `q=` vs base list vs stats). */
function mockFetchByUrl(handler: (url: string) => Response) {
  vi.spyOn(globalThis, "fetch").mockImplementation((input) => Promise.resolve(handler(String(input))));
}

const emptyStats = jsonResponse({
  aggregates: {
    total: 0, first_ts: 0, last_ts: 0, bots: 0,
    per_day: {}, per_country: {}, per_device: {}, per_os: {}, per_browser: {}, per_referer: {}, per_city: {}, per_variant: {},
  },
  recent: [],
});

describe("Analytics", () => {
  beforeEach(() => { localStorage.setItem("quark_admin_token", "s"); vi.restoreAllMocks(); });

  it("shows the empty state and no stats content when nothing is selected", async () => {
    mockFetchByUrl(() => jsonResponse({ links: [], next_after: null }));
    render(withProviders(<Analytics />));
    expect(await screen.findByText(/select a link to see its analytics/i)).toBeInTheDocument();
    expect(screen.queryByText(/total clicks/i)).not.toBeInTheDocument();
  });

  it("lists matching links as the user types in the search box", async () => {
    const base = {
      links: [
        { id: 1, code: "AAA0000", url: "https://cat.com", expiry: null, created: 1, rules: [], variants: [] },
        { id: 2, code: "BBB1111", url: "https://dog.com", expiry: null, created: 2, rules: [], variants: [] },
      ],
      next_after: null,
    };
    mockFetchByUrl((url) => {
      if (url.includes("/stats")) return emptyStats;
      return url.includes("q=")
        ? jsonResponse({ links: base.links.filter((l) => l.url.includes("dog")), next_after: null })
        : jsonResponse(base);
    });
    render(withProviders(<Analytics />));
    await screen.findByText("AAA0000");
    await userEvent.type(screen.getByRole("searchbox"), "dog");
    await waitFor(() => {
      expect(screen.getByText("BBB1111")).toBeInTheDocument();
      expect(screen.queryByText("AAA0000")).not.toBeInTheDocument();
    });
  });

  it("clicking a result renders that link's stats", async () => {
    const base = {
      links: [{ id: 1, code: "AAA0000", url: "https://cat.com", expiry: null, created: 1, rules: [], variants: [] }],
      next_after: null,
    };
    mockFetchByUrl((url) => {
      if (url.includes("/AAA0000/stats")) {
        return jsonResponse({
          aggregates: {
            total: 42, first_ts: 1700000000, last_ts: 1700100000, bots: 0,
            per_day: {}, per_country: {}, per_device: {}, per_os: {}, per_browser: {}, per_referer: {}, per_city: {}, per_variant: {},
          },
          recent: [],
        });
      }
      return jsonResponse(base);
    });
    render(withProviders(<Analytics />));
    const result = await screen.findByRole("button", { name: /AAA0000/i });
    await userEvent.click(result);
    expect(await screen.findByText("42")).toBeInTheDocument();
  });

  it("picking a different result swaps the stats without a full reload", async () => {
    const base = {
      links: [
        { id: 1, code: "AAA0000", url: "https://cat.com", expiry: null, created: 1, rules: [], variants: [] },
        { id: 2, code: "BBB1111", url: "https://dog.com", expiry: null, created: 2, rules: [], variants: [] },
      ],
      next_after: null,
    };
    mockFetchByUrl((url) => {
      if (url.includes("/AAA0000/stats")) {
        return jsonResponse({
          aggregates: {
            total: 10, first_ts: 1, last_ts: 2, bots: 0,
            per_day: {}, per_country: {}, per_device: {}, per_os: {}, per_browser: {}, per_referer: {}, per_city: {}, per_variant: {},
          },
          recent: [],
        });
      }
      if (url.includes("/BBB1111/stats")) {
        return jsonResponse({
          aggregates: {
            total: 20, first_ts: 1, last_ts: 2, bots: 0,
            per_day: {}, per_country: {}, per_device: {}, per_os: {}, per_browser: {}, per_referer: {}, per_city: {}, per_variant: {},
          },
          recent: [],
        });
      }
      return jsonResponse(base);
    });
    render(withProviders(<Analytics />));
    await userEvent.click(await screen.findByRole("button", { name: /AAA0000/i }));
    expect(await screen.findByText("10")).toBeInTheDocument();

    await userEvent.click(await screen.findByRole("button", { name: /BBB1111/i }));
    expect(await screen.findByText("20")).toBeInTheDocument();
    expect(screen.queryByText("10")).not.toBeInTheDocument();
  });

  it("falls back to client-side filtering when the search returns 501", async () => {
    const base = {
      links: [
        { id: 1, code: "AAA0000", url: "https://github.com/x", expiry: null, created: 1, rules: [], variants: [] },
        { id: 2, code: "BBB1111", url: "https://example.com", expiry: null, created: 2, rules: [], variants: [] },
      ],
      next_after: null,
    };
    mockFetchByUrl((url) => {
      if (url.includes("/stats")) return emptyStats;
      return url.includes("q=") ? new Response("{}", { status: 501 }) : jsonResponse(base);
    });
    render(withProviders(<Analytics />));
    await screen.findByText("AAA0000");
    await userEvent.type(screen.getByRole("searchbox"), "github");
    await waitFor(() => {
      expect(screen.getByText("AAA0000")).toBeInTheDocument();
      expect(screen.queryByText("BBB1111")).not.toBeInTheDocument();
    });
  });

  it("shows the no-results message when the search yields nothing", async () => {
    const base = { links: [{ id: 1, code: "AAA0000", url: "https://cat.com", expiry: null, created: 1, rules: [], variants: [] }], next_after: null };
    mockFetchByUrl((url) =>
      url.includes("q=") ? jsonResponse({ links: [], next_after: null }) : jsonResponse(base),
    );
    render(withProviders(<Analytics />));
    await screen.findByText("AAA0000");
    await userEvent.type(screen.getByRole("searchbox"), "zzz");
    expect(await screen.findByText(/no links found/i)).toBeInTheDocument();
  });
});
