import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nProvider } from "@/i18n";
import { Extensions } from "./Extensions";
import type { SheetsStatus } from "@/lib/types";

/** Renders the catalog inside a router where /webhooks and /pixels resolve to markers, so navigation is observable. */
function renderCatalog() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <I18nProvider locale="en">
      <QueryClientProvider client={qc}>
        <MemoryRouter initialEntries={["/extensions"]}>
          <Routes>
            <Route path="/extensions" element={<Extensions />} />
            <Route path="/webhooks" element={<div>webhooks page</div>} />
            <Route path="/pixels" element={<div>pixels page</div>} />
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>
    </I18nProvider>,
  );
}

/**
 * Mocks the Sheets status endpoint (`GET /admin/integrations/sheets/status`).
 * `status` is the HTTP status (401/404 model the connector being off); `body`
 * the JSON payload when 200.
 */
function mockSheetsFetch(status: number, body?: SheetsStatus) {
  return vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
    const url = String(typeof input === "string" ? input : (input as Request).url ?? input);
    if (url.includes("/admin/integrations/sheets/status")) {
      return new Response(status === 200 ? JSON.stringify(body) : "", { status });
    }
    if (url.includes("/admin/integrations/sheets/sync")) {
      return new Response(JSON.stringify({ ...body, last_status: { state: "ok" } }), { status: 200 });
    }
    return new Response("", { status: 404, statusText: `unexpected ${url} ${init?.method ?? "GET"}` });
  });
}

describe("Extensions", () => {
  // Default: the connector is off (404). The Sheets card then renders its
  // "via Webhooks" fallback, matching the pre-connector behavior the catalog
  // tests below assert against.
  beforeEach(() => { mockSheetsFetch(404); });
  afterEach(() => { vi.restoreAllMocks(); });

  it("renders the catalog with every integration", () => {
    renderCatalog();
    expect(screen.getByRole("heading", { level: 1, name: /extensions/i })).toBeInTheDocument();
    // A sample across each category.
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

  it("marks not-yet-built integrations as coming soon and does not let them navigate", () => {
    renderCatalog();
    // Four coming-soon items: GTM, TikTok, LinkedIn, Notion. Each shows a badge and a disabled button.
    expect(screen.getAllByText(/coming soon/i).length).toBeGreaterThanOrEqual(4);
    for (const button of screen.getAllByRole("button", { name: /coming soon/i })) {
      expect(button).toBeDisabled();
    }
  });

  it("navigates a webhooks-powered card to /webhooks", async () => {
    renderCatalog();
    const buttons = screen.getAllByRole("button", { name: /set up via webhooks/i });
    expect(buttons.length).toBeGreaterThan(0);
    await userEvent.click(buttons[0]);
    expect(await screen.findByText("webhooks page")).toBeInTheDocument();
  });

  it("navigates a pixels-powered card to /pixels", async () => {
    renderCatalog();
    const buttons = screen.getAllByRole("button", { name: /set up via pixels/i });
    expect(buttons.length).toBeGreaterThan(0);
    await userEvent.click(buttons[0]);
    expect(await screen.findByText("pixels page")).toBeInTheDocument();
  });

  it("Sheets card shows Connect when the connector is on but not connected", async () => {
    mockSheetsFetch(200, { connected: false, last_status: { state: "never" } });
    renderCatalog();
    expect(await screen.findByRole("button", { name: /connect google sheets/i })).toBeInTheDocument();
  });

  it("Sheets card shows the connected email and a Sync now button when connected", async () => {
    mockSheetsFetch(200, {
      connected: true,
      email: "ops@example.com",
      spreadsheet_url: "https://docs.google.com/spreadsheets/d/abc",
      last_sync: 1700000000,
      last_status: { state: "ok" },
    });
    renderCatalog();
    expect(await screen.findByText(/connected as ops@example.com/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /sync now/i })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /open spreadsheet/i })).toHaveAttribute(
      "href",
      "https://docs.google.com/spreadsheets/d/abc",
    );
  });

  it("clicking Sync now calls the sync endpoint", async () => {
    const fetchMock = mockSheetsFetch(200, {
      connected: true,
      email: "ops@example.com",
      last_sync: 1700000000,
      last_status: { state: "ok" },
    });
    renderCatalog();
    await userEvent.click(await screen.findByRole("button", { name: /sync now/i }));
    await vi.waitFor(() => {
      const call = fetchMock.mock.calls.find(([url, opts]) =>
        String(url).includes("/admin/integrations/sheets/sync") && (opts as RequestInit | undefined)?.method === "POST",
      );
      if (!call) throw new Error("POST /admin/integrations/sheets/sync not called yet");
    });
  });
});
