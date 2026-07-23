import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nProvider } from "@/i18n";
import { ExtensionDetail } from "./ExtensionDetail";
import type { SheetsStatus } from "@/lib/types";

/** Renders the detail view for `id`, with marker routes so navigation is observable. */
function renderDetail(id: string) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <I18nProvider locale="en">
      <QueryClientProvider client={qc}>
        <MemoryRouter initialEntries={[`/extensions/${id}`]}>
          <Routes>
            <Route path="/extensions" element={<div>catalog page</div>} />
            <Route path="/extensions/:id" element={<ExtensionDetail />} />
            <Route path="/webhooks" element={<div>webhooks page</div>} />
            <Route path="/pixels" element={<div>pixels page</div>} />
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>
    </I18nProvider>,
  );
}

/** Base status mock: Sheets off (404), no webhooks, no pixels. Extra handlers can be layered by the caller's own spy. */
function mockBase(opts: { sheetsStatus?: number; sheetsBody?: SheetsStatus; slackConnect?: boolean; webhooks?: { id: number; kind: string; url: string; label?: string }[] } = {}) {
  return vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
    const url = String(typeof input === "string" ? input : (input as Request).url ?? input);
    const method = (init as RequestInit | undefined)?.method ?? "GET";
    if (url.includes("/admin/me")) {
      return new Response(JSON.stringify({ authenticated: true, slack_connect: opts.slackConnect ?? false }), { status: 200 });
    }
    if (url.includes("/admin/integrations/sheets/status")) {
      const s = opts.sheetsStatus ?? 404;
      return new Response(s === 200 ? JSON.stringify(opts.sheetsBody) : "", { status: s });
    }
    if (url.includes("/admin/integrations/sheets/sync") && method === "POST") {
      return new Response(JSON.stringify({ ...opts.sheetsBody, last_status: { state: "ok" } }), { status: 200 });
    }
    if (url.includes("/admin/integrations/slack/connect")) {
      return new Response(JSON.stringify({ url: "https://slack.com/oauth/v2/authorize?x=1" }), { status: 200 });
    }
    if (url.includes("/admin/webhooks/") && method === "DELETE") return new Response("", { status: 204 });
    if (url.includes("/admin/webhooks") && method === "GET") return new Response(JSON.stringify({ webhooks: opts.webhooks ?? [] }), { status: 200 });
    if (url.includes("/admin/pixels") && method === "GET") return new Response(JSON.stringify({ pixels: [] }), { status: 200 });
    if (url.includes("/admin/webhooks") && method === "POST") return new Response(JSON.stringify({ id: 1, secret: "" }), { status: 201 });
    if (url.includes("/admin/pixels") && method === "POST") return new Response(JSON.stringify({ id: 1 }), { status: 201 });
    return new Response("", { status: 404, statusText: `unexpected ${url} ${method}` });
  });
}

describe("ExtensionDetail", () => {
  afterEach(() => { vi.restoreAllMocks(); });

  it("redirects unknown ids back to the catalog", () => {
    mockBase();
    renderDetail("does-not-exist");
    expect(screen.getByText("catalog page")).toBeInTheDocument();
  });

  it("renders the integration header (name + description)", async () => {
    mockBase();
    renderDetail("slack");
    expect(screen.getByRole("heading", { level: 1, name: "Slack" })).toBeInTheDocument();
  });

  it("creates a webhook inline with the integration's fixed kind", async () => {
    const fetchMock = mockBase();
    renderDetail("slack");

    await userEvent.type(await screen.findByLabelText(/webhook url/i), "https://hooks.slack.com/services/x");
    await userEvent.click(screen.getByRole("button", { name: /^activate$/i }));

    const call = await vi.waitFor(() => {
      const c = fetchMock.mock.calls.find(
        ([u, o]) => String(u).includes("/admin/webhooks") && (o as RequestInit | undefined)?.method === "POST",
      );
      if (!c) throw new Error("POST /admin/webhooks not called yet");
      return c;
    });
    const body = JSON.parse(String((call[1] as RequestInit).body));
    expect(body.kind).toBe("slack");
    expect(body.url).toBe("https://hooks.slack.com/services/x");
    expect(body.events).toHaveLength(6);
  });

  it("creates a pixel inline with the integration's fixed provider", async () => {
    const fetchMock = mockBase();
    renderDetail("ga4");

    await userEvent.type(await screen.findByLabelText(/measurement id/i), "G-ABC123");
    await userEvent.type(screen.getByLabelText(/api secret/i), "s3cr3t");
    await userEvent.click(screen.getByRole("button", { name: /^activate$/i }));

    const call = await vi.waitFor(() => {
      const c = fetchMock.mock.calls.find(
        ([u, o]) => String(u).includes("/admin/pixels") && (o as RequestInit | undefined)?.method === "POST",
      );
      if (!c) throw new Error("POST /admin/pixels not called yet");
      return c;
    });
    const body = JSON.parse(String((call[1] as RequestInit).body));
    expect(body).toEqual({ provider: "ga4", credentials: { measurement_id: "G-ABC123", api_secret: "s3cr3t" } });
  });

  it("shows the connected email and a Sync now button when Sheets is connected", async () => {
    mockBase({
      sheetsStatus: 200,
      sheetsBody: {
        connected: true,
        email: "ops@example.com",
        spreadsheet_url: "https://docs.google.com/spreadsheets/d/abc",
        last_sync: 1700000000,
        last_status: { state: "ok" },
      },
    });
    renderDetail("sheets");
    expect(await screen.findByText(/connected as ops@example.com/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /sync now/i })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /open spreadsheet/i })).toHaveAttribute(
      "href",
      "https://docs.google.com/spreadsheets/d/abc",
    );
  });

  it("offers Add to Slack when the OAuth connector is configured, and starts the install on click", async () => {
    const fetchMock = mockBase({ slackConnect: true });
    renderDetail("slack");

    const addBtn = await screen.findByRole("button", { name: /add to slack/i });
    // The manual form still exists as a fallback, under the "or set up manually" label.
    expect(screen.getByText(/or set up manually/i)).toBeInTheDocument();

    await userEvent.click(addBtn);
    await vi.waitFor(() => {
      const c = fetchMock.mock.calls.find(([u]) => String(u).includes("/admin/integrations/slack/connect"));
      if (!c) throw new Error("GET /admin/integrations/slack/connect not called yet");
    });
  });

  it("shows only the manual webhook form when the Slack OAuth connector is off", async () => {
    mockBase({ slackConnect: false });
    renderDetail("slack");
    expect(await screen.findByLabelText(/webhook url/i)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /add to slack/i })).not.toBeInTheDocument();
  });

  it("shows the connected channel with a disconnect action when a Slack subscription exists", async () => {
    const fetchMock = mockBase({
      slackConnect: true,
      webhooks: [{ id: 7, kind: "slack", url: "https://hooks.slack.com/services/T/B/secret", label: "#general" }],
    });
    renderDetail("slack");

    // Connected state is shown; the OAuth CTA is demoted to "add another channel".
    expect(await screen.findByText(/slack is connected/i)).toBeInTheDocument();
    // The channel name identifies the connection (not the opaque URL).
    expect(screen.getByText("#general")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /add another channel/i })).toBeInTheDocument();
    // The raw token is never rendered in full.
    expect(screen.queryByText(/secret/)).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /disconnect/i }));
    await vi.waitFor(() => {
      const c = fetchMock.mock.calls.find(
        ([u, o]) => String(u).includes("/admin/webhooks/7") && (o as RequestInit | undefined)?.method === "DELETE",
      );
      if (!c) throw new Error("DELETE /admin/webhooks/7 not called yet");
    });
  });

  it("shows an unavailable notice for Sheets when the connector is off (no Webhooks fallback)", async () => {
    mockBase({ sheetsStatus: 404 });
    renderDetail("sheets");
    expect(await screen.findByText(/isn't set up on this quark instance/i)).toBeInTheDocument();
    // The old confusing "via Webhooks" affordance is gone.
    expect(screen.queryByText(/via webhooks/i)).not.toBeInTheDocument();
    expect(screen.queryByText("webhooks page")).not.toBeInTheDocument();
  });
});
