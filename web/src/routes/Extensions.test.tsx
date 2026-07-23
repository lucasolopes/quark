import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nProvider } from "@/i18n";
import { Extensions } from "./Extensions";
import type { SheetsStatus } from "@/lib/types";

/** Renders the catalog inside a router where /extensions/:id resolves to a marker, so navigation is observable. */
function renderCatalog() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <I18nProvider locale="en">
      <QueryClientProvider client={qc}>
        <MemoryRouter initialEntries={["/extensions"]}>
          <Routes>
            <Route path="/extensions" element={<Extensions />} />
            <Route path="/extensions/:id" element={<div>detail page</div>} />
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>
    </I18nProvider>,
  );
}

/**
 * Mocks the connection-status endpoints the catalog reads. `sheetsStatus` is
 * the HTTP status for the Sheets status endpoint (401/404 model the connector
 * being off). Webhooks and pixels list endpoints return the given arrays.
 */
function mockStatusFetch(opts: {
  sheetsStatus: number;
  sheetsBody?: SheetsStatus;
  webhooks?: { kind: string }[];
  pixels?: { provider: string }[];
} = { sheetsStatus: 404 }) {
  return vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
    const url = String(typeof input === "string" ? input : (input as Request).url ?? input);
    if (url.includes("/admin/integrations/sheets/status")) {
      return new Response(opts.sheetsStatus === 200 ? JSON.stringify(opts.sheetsBody) : "", { status: opts.sheetsStatus });
    }
    if (url.includes("/admin/webhooks")) {
      return new Response(JSON.stringify({ webhooks: opts.webhooks ?? [] }), { status: 200 });
    }
    if (url.includes("/admin/pixels")) {
      return new Response(JSON.stringify({ pixels: opts.pixels ?? [] }), { status: 200 });
    }
    return new Response("", { status: 404, statusText: `unexpected ${url}` });
  });
}

describe("Extensions", () => {
  beforeEach(() => { mockStatusFetch(); });
  afterEach(() => { vi.restoreAllMocks(); });

  it("renders the catalog with every integration", () => {
    renderCatalog();
    expect(screen.getByRole("heading", { level: 1, name: /extensions/i })).toBeInTheDocument();
    expect(screen.getByText("Slack")).toBeInTheDocument();
    expect(screen.getByText("Zapier")).toBeInTheDocument();
    expect(screen.getByText("GA4 Measurement")).toBeInTheDocument();
    expect(screen.getByText("Notion")).toBeInTheDocument();
  });

  it("groups integrations by category with eyebrow headers", () => {
    renderCatalog();
    for (const label of [/automation/i, /notifications/i, /analytics/i, /dev & data/i]) {
      expect(screen.getByRole("heading", { level: 2, name: label })).toBeInTheDocument();
    }
  });

  it("marks not-yet-built integrations as coming soon and does not link them", () => {
    renderCatalog();
    // Notion is a coming-soon integration: it shows the badge and is not a link.
    const notionCard = screen.getByText("Notion").closest('[data-slot="card"]') as HTMLElement;
    expect(within(notionCard).getByText(/coming soon/i)).toBeInTheDocument();
    expect(notionCard.closest("a")).toBeNull();
  });

  it("links each connectable integration to its dedicated view", async () => {
    renderCatalog();
    const slackLink = screen.getByText("Slack").closest("a") as HTMLElement;
    expect(slackLink).toHaveAttribute("href", "/extensions/slack");
    await userEvent.click(slackLink);
    expect(await screen.findByText("detail page")).toBeInTheDocument();
  });

  it("shows a Connected badge on a card whose backing resource exists", async () => {
    mockStatusFetch({ sheetsStatus: 404, webhooks: [{ kind: "slack" }] });
    renderCatalog();
    const slackCard = screen.getByText("Slack").closest('[data-slot="card"]') as HTMLElement;
    expect(await within(slackCard).findByText(/connected/i)).toBeInTheDocument();
    // A card without a backing resource does not show the badge.
    const discordCard = screen.getByText("Discord").closest('[data-slot="card"]') as HTMLElement;
    expect(within(discordCard).queryByText(/connected/i)).not.toBeInTheDocument();
  });

  it("shows the Connected badge on the Sheets card when connected", async () => {
    mockStatusFetch({ sheetsStatus: 200, sheetsBody: { connected: true, email: "ops@example.com", last_status: { state: "ok" } } });
    renderCatalog();
    const sheetsCard = screen.getByText("Google Sheets").closest('[data-slot="card"]') as HTMLElement;
    expect(await within(sheetsCard).findByText(/connected/i)).toBeInTheDocument();
  });
});
