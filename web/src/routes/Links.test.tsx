import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useSearchParams } from "react-router-dom";
import { Links } from "./Links";
import { withProviders } from "@/test-utils";

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status });
}

/** Mocks `fetch`, responding based on which URL was called (search `q=` vs base list). */
function mockFetchByUrl(handler: (url: string) => Response) {
  vi.spyOn(globalThis, "fetch").mockImplementation((input) => Promise.resolve(handler(String(input))));
}

/** Renders the current query string so URL param cleanup (e.g. `?new=1`) is observable. */
function SearchParamsProbe() {
  const [params] = useSearchParams();
  return <div data-testid="probe">{params.toString()}</div>;
}

describe("Links", () => {
  beforeEach(() => { localStorage.setItem("quark_admin_token", "s"); vi.restoreAllMocks(); });

  it("renders the loaded links", async () => {
    mockFetchByUrl(() => jsonResponse({
      links: [{ id: 1, code: "6lB362J", url: "https://example.com/a", alias: "promo", expiry: null, created: 1700000000, rules: [], variants: [] }],
      next_after: null,
    }));
    render(withProviders(<Links />));
    expect(await screen.findByText("6lB362J")).toBeInTheDocument();
    expect(screen.getByText(/example\.com\/a/)).toBeInTheDocument();
  });

  it("hides write actions for a Viewer (no links_write scope)", async () => {
    // No break-glass token, so `useMe` runs and reports the Viewer's scopes.
    localStorage.removeItem("quark_admin_token");
    mockFetchByUrl((url) => {
      if (url.includes("/admin/me")) {
        return jsonResponse({
          authenticated: true,
          oidc_enabled: true,
          multi_tenant: true,
          scopes: ["links_read", "analytics"],
          memberships: [{ tenant_id: 1, name: "W", slug: "w", role: "viewer" }],
          current_tenant: 1,
        });
      }
      if (url.includes("/admin/tags")) return jsonResponse({ tags: [] });
      if (url.includes("/admin/folders")) return jsonResponse({ folders: [] });
      return jsonResponse({
        links: [{ id: 1, code: "6lB362J", url: "https://example.com/a", expiry: null, created: 1700000000, rules: [], variants: [] }],
        next_after: null,
      });
    });
    render(withProviders(<Links />));
    // The list still renders (Viewer can read)…
    expect(await screen.findByText("6lB362J")).toBeInTheDocument();
    // …but no create affordance, and no per-row bulk-select checkbox.
    expect(screen.queryByRole("button", { name: /create link/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("checkbox", { name: /select/i })).not.toBeInTheDocument();
  });

  it("searches on the server when the backend supports it (?q= in the querystring)", async () => {
    const base = {
      links: [
        { id: 1, code: "AAA0000", url: "https://cat.com", expiry: null, created: 1, rules: [], variants: [] },
        { id: 2, code: "BBB1111", url: "https://dog.com", expiry: null, created: 2, rules: [], variants: [] },
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
        { id: 1, code: "AAA0000", url: "https://github.com/x", expiry: null, created: 1, rules: [], variants: [] },
        { id: 2, code: "BBB1111", url: "https://example.com", expiry: null, created: 2, rules: [], variants: [] },
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
    const base = { links: [{ id: 1, code: "AAA0000", url: "https://cat.com", expiry: null, created: 1, rules: [], variants: [] }], next_after: null };
    mockFetchByUrl((url) =>
      url.includes("q=") ? jsonResponse({ links: [], next_after: null }) : jsonResponse(base),
    );
    render(withProviders(<Links />));
    await screen.findByText("AAA0000");
    await userEvent.type(screen.getByRole("searchbox"), "zzz");
    expect(await screen.findByText(/no links found for "zzz"/i)).toBeInTheDocument();
  });

  it("search error (non-501, 500) shows the error state, not the 'no results' one", async () => {
    const base = { links: [{ id: 1, code: "AAA0000", url: "https://cat.com", expiry: null, created: 1, rules: [], variants: [] }], next_after: null };
    mockFetchByUrl((url) => (url.includes("q=") ? new Response("{}", { status: 500 }) : jsonResponse(base)));
    render(withProviders(<Links />));
    await screen.findByText("AAA0000");
    await userEvent.type(screen.getByRole("searchbox"), "zzz");
    expect(await screen.findByText(/could not search/i)).toBeInTheDocument();
    expect(screen.queryByText(/no links found for "zzz"/i)).not.toBeInTheDocument();
  });

  it("empty state, whose CTA opens the create dialog (the PageHeader's duplicate button was removed)", async () => {
    mockFetchByUrl(() => jsonResponse({ links: [], next_after: null }));
    render(withProviders(<Links />));
    expect(await screen.findByText(/no links yet/i)).toBeInTheDocument();

    // Exactly one "Create link" affordance remains on the page (the empty-state CTA).
    await userEvent.click(screen.getByRole("button", { name: /^create link$/i }));
    expect(await screen.findByRole("dialog", { name: /create link/i })).toBeInTheDocument();
  });

  it("filters by folder via the API (?folder= in the querystring)", async () => {
    const base = {
      links: [
        { id: 1, code: "AAA0000", url: "https://cat.com", expiry: null, created: 1, folder: "Docs", rules: [], variants: [] },
        { id: 2, code: "BBB1111", url: "https://dog.com", expiry: null, created: 2, folder: "Marketing", rules: [], variants: [] },
      ],
      next_after: null,
    };
    const calledUrls: string[] = [];
    mockFetchByUrl((url) => {
      calledUrls.push(url);
      if (url.includes("/admin/folders"))
        return jsonResponse({ folders: [{ name: "Docs", count: 1 }, { name: "Marketing", count: 1 }] });
      if (url.includes("folder=Marketing"))
        return jsonResponse({ links: base.links.filter((l) => l.folder === "Marketing"), next_after: null });
      return jsonResponse(base);
    });
    render(withProviders(<Links />));
    await screen.findByText("AAA0000");

    await userEvent.selectOptions(screen.getByRole("combobox", { name: /filter by folder/i }), "Marketing");

    await waitFor(() => {
      expect(screen.getByText("BBB1111")).toBeInTheDocument();
      expect(screen.queryByText("AAA0000")).not.toBeInTheDocument();
    });
    expect(calledUrls.some((u) => u.includes("folder=Marketing"))).toBe(true);
  });

  it("filters by active status via the API (?status=active in the querystring)", async () => {
    const base = {
      links: [
        { id: 1, code: "AAA0000", url: "https://cat.com", expiry: null, created: 1, rules: [], variants: [] },
        { id: 2, code: "BBB1111", url: "https://dog.com", expiry: null, created: 2, rules: [], variants: [] },
      ],
      next_after: null,
    };
    const calledUrls: string[] = [];
    mockFetchByUrl((url) => {
      calledUrls.push(url);
      if (url.includes("status=active"))
        return jsonResponse({ links: base.links.filter((l) => l.code === "AAA0000"), next_after: null });
      return jsonResponse(base);
    });
    render(withProviders(<Links />));
    await screen.findByText("AAA0000");

    await userEvent.click(screen.getByRole("checkbox", { name: /active only/i }));

    await waitFor(() => {
      expect(screen.getByText("AAA0000")).toBeInTheDocument();
      expect(screen.queryByText("BBB1111")).not.toBeInTheDocument();
    });
    expect(calledUrls.some((u) => u.includes("status=active"))).toBe(true);
  });

  it("filters by tag via the API (?tag= in the querystring)", async () => {
    const base = {
      links: [
        { id: 1, code: "AAA0000", url: "https://cat.com", expiry: null, created: 1, tags: ["pets"], rules: [], variants: [] },
        { id: 2, code: "BBB1111", url: "https://dog.com", expiry: null, created: 2, tags: ["pets", "promo"], rules: [], variants: [] },
      ],
      next_after: null,
    };
    mockFetchByUrl((url) => {
      if (url.includes("/admin/tags"))
        return jsonResponse({ tags: [{ name: "pets", count: 2 }, { name: "promo", count: 1 }] });
      if (url.includes("tag=promo"))
        return jsonResponse({ links: base.links.filter((l) => l.tags.includes("promo")), next_after: null });
      return jsonResponse(base);
    });
    render(withProviders(<Links />));
    await screen.findByText("AAA0000");

    // The tag filter is now a row of pill chips (mock isTabLinks.html); click the
    // "promo" chip instead of selecting a <select> option.
    await userEvent.click(await screen.findByRole("button", { name: /promo/i }));

    await waitFor(() => {
      expect(screen.getByText("BBB1111")).toBeInTheDocument();
      expect(screen.queryByText("AAA0000")).not.toBeInTheDocument();
    });
  });

  it("caps tag chips at 10 behind a '+N' toggle that expands to reveal every tag (still filterable)", async () => {
    const tagNames = [
      "alfa", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel",
      "india", "juliet", "kilo", "lima", "mike", "november",
    ];
    const base = {
      links: [
        { id: 1, code: "AAA0000", url: "https://cat.com", expiry: null, created: 1, tags: ["november"], rules: [], variants: [] },
        { id: 2, code: "BBB1111", url: "https://dog.com", expiry: null, created: 2, tags: ["alfa"], rules: [], variants: [] },
      ],
      next_after: null,
    };
    mockFetchByUrl((url) => {
      if (url.includes("/admin/tags"))
        return jsonResponse({ tags: tagNames.map((name, i) => ({ name, count: i + 1 })) });
      if (url.includes("tag=november"))
        return jsonResponse({ links: base.links.filter((l) => l.tags.includes("november")), next_after: null });
      return jsonResponse(base);
    });

    render(withProviders(<Links />));
    const group = await screen.findByRole("group", { name: /filter by tag/i });

    // 10-tag cap: the first 10 alphabetic chips render; "november" (14th) stays behind "+4".
    expect(within(group).getByRole("button", { name: /^alfa/i })).toBeInTheDocument();
    expect(within(group).queryByRole("button", { name: /^november/i })).not.toBeInTheDocument();
    const moreButton = within(group).getByRole("button", { name: "+4" });

    await userEvent.click(moreButton);

    // Expanded: all 14 tags show, including "november" past the original cap, and the
    // same slot now reads "Show less".
    const novemberChip = within(group).getByRole("button", { name: /^november/i });
    expect(within(group).getByRole("button", { name: /show less/i })).toBeInTheDocument();
    expect(within(group).queryByRole("button", { name: "+4" })).not.toBeInTheDocument();

    // Every tag is still filterable once expanded, including ones beyond the original cap.
    await userEvent.click(novemberChip);
    await waitFor(() => {
      expect(screen.getByText("AAA0000")).toBeInTheDocument();
      expect(screen.queryByText("BBB1111")).not.toBeInTheDocument();
    });
  });

  it("round-trip: expand tag chips, then collapse back to the capped view (+N button returns)", async () => {
    const tagNames = [
      "alfa", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel",
      "india", "juliet", "kilo", "lima", "mike", "november",
    ];
    const base = {
      links: [{ id: 1, code: "AAA0000", url: "https://cat.com", expiry: null, created: 1, tags: [], rules: [], variants: [] }],
      next_after: null,
    };
    mockFetchByUrl((url) => {
      if (url.includes("/admin/tags"))
        return jsonResponse({ tags: tagNames.map((name, i) => ({ name, count: i + 1 })) });
      return jsonResponse(base);
    });

    render(withProviders(<Links />));
    const group = await screen.findByRole("group", { name: /filter by tag/i });

    // Start collapsed: only the first 10, plus "+4" button.
    expect(within(group).getByRole("button", { name: "+4" })).toBeInTheDocument();
    expect(within(group).queryByRole("button", { name: /show less/i })).not.toBeInTheDocument();

    // Expand.
    await userEvent.click(within(group).getByRole("button", { name: "+4" }));
    expect(within(group).getByRole("button", { name: /show less/i })).toBeInTheDocument();
    expect(within(group).queryByRole("button", { name: "+4" })).not.toBeInTheDocument();

    // Collapse.
    await userEvent.click(within(group).getByRole("button", { name: /show less/i }));
    expect(within(group).getByRole("button", { name: "+4" })).toBeInTheDocument();
    expect(within(group).queryByRole("button", { name: /show less/i })).not.toBeInTheDocument();
  });

  it("pins the active tag when collapsed if it's beyond the first 10, so the selection stays visible", async () => {
    const tagNames = [
      "alfa", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel",
      "india", "juliet", "kilo", "lima", "mike", "november",
    ];
    const base = {
      links: [
        { id: 1, code: "AAA0000", url: "https://cat.com", expiry: null, created: 1, tags: ["november"], rules: [], variants: [] },
        { id: 2, code: "BBB1111", url: "https://dog.com", expiry: null, created: 2, tags: ["alfa"], rules: [], variants: [] },
      ],
      next_after: null,
    };
    mockFetchByUrl((url) => {
      if (url.includes("/admin/tags"))
        return jsonResponse({ tags: tagNames.map((name, i) => ({ name, count: i + 1 })) });
      if (url.includes("tag=november"))
        return jsonResponse({ links: base.links.filter((l) => l.tags.includes("november")), next_after: null });
      return jsonResponse(base);
    });

    render(withProviders(<Links />));
    const group = await screen.findByRole("group", { name: /filter by tag/i });

    // Start collapsed: "november" (14th) is hidden behind "+4".
    expect(within(group).queryByRole("button", { name: /^november/i })).not.toBeInTheDocument();

    // Expand and select "november" (beyond the 10-chip cap).
    await userEvent.click(within(group).getByRole("button", { name: "+4" }));
    const novemberChip = within(group).getByRole("button", { name: /^november/i });
    await userEvent.click(novemberChip);

    // Filter is now active; collapse via the "Show less" button.
    await userEvent.click(within(group).getByRole("button", { name: /show less/i }));

    // The "november" chip is pinned in the visible set, marked as pressed, and
    // the "+N" count reflects one fewer hidden (13 instead of 14).
    const novemberChipAfterCollapse = within(group).getByRole("button", { name: /^november/i });
    expect(novemberChipAfterCollapse).toHaveAttribute("aria-pressed", "true");
    expect(within(group).getByRole("button", { name: "+3" })).toBeInTheDocument();
  });


  it("pre-fills the search from ?q= on mount and runs the server search", async () => {
    const base = {
      links: [
        { id: 1, code: "AAA0000", url: "https://cat.com", expiry: null, created: 1, rules: [], variants: [] },
        { id: 2, code: "BBB1111", url: "https://dog.com", expiry: null, created: 2, rules: [], variants: [] },
      ],
      next_after: null,
    };
    mockFetchByUrl((url) =>
      url.includes("q=")
        ? jsonResponse({ links: base.links.filter((l) => l.url.includes("dog")), next_after: null })
        : jsonResponse(base),
    );
    render(withProviders(<Links />, { initialEntries: ["/links?q=dog"] }));

    expect(screen.getByRole("searchbox")).toHaveValue("dog");
    await waitFor(() => {
      expect(screen.getByText("BBB1111")).toBeInTheDocument();
      expect(screen.queryByText("AAA0000")).not.toBeInTheDocument();
    });
  });

  it("opens the create dialog from ?new=1 and strips the param from the URL", async () => {
    mockFetchByUrl(() => jsonResponse({ links: [], next_after: null }));
    render(
      withProviders(
        <>
          <Links />
          <SearchParamsProbe />
        </>,
        { initialEntries: ["/links?new=1"] },
      ),
    );

    expect(await screen.findByRole("dialog", { name: /create link/i })).toBeInTheDocument();
    // The param is consumed on mount (replace), so a refresh / close won't reopen it.
    await waitFor(() => expect(screen.getByTestId("probe").textContent).toBe(""));
  });
});
