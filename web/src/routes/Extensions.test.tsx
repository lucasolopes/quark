import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nProvider } from "@/i18n";
import { Extensions } from "./Extensions";

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

describe("Extensions", () => {
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
});
