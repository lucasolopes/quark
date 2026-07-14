import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { RecentEventsTable } from "./RecentEventsTable";
import { withProviders } from "@/test-utils";
import type { ClickEvent } from "@/lib/types";

describe("RecentEventsTable — city column", () => {
  it("shows the city header and the event's city when present", () => {
    const events: ClickEvent[] = [
      { id: 1, ts: 1700000000, country: "BR", referer: "https://news.ycombinator.com/x", city: "Sao Paulo" },
    ];
    render(withProviders(<RecentEventsTable events={events} />, { withRouter: false }));

    expect(screen.getByRole("columnheader", { name: /city/i })).toBeInTheDocument();
    expect(screen.getByText("Sao Paulo")).toBeInTheDocument();
  });

  it("falls back to a dash when the event has no city", () => {
    const events: ClickEvent[] = [{ id: 1, ts: 1700000000, country: "BR", referer: null, city: null }];
    render(withProviders(<RecentEventsTable events={events} />, { withRouter: false }));

    const row = screen.getByRole("row", { name: /BR/i });
    expect(row.textContent).toContain("—");
  });
});

describe("RecentEventsTable — bot badge", () => {
  it("shows a bot badge on rows flagged as bot", () => {
    const events: ClickEvent[] = [
      { id: 1, ts: 1700000000, country: "US", referer: null, city: null, bot: true },
    ];
    render(withProviders(<RecentEventsTable events={events} />, { withRouter: false }));

    expect(screen.getByText("Bot")).toBeInTheDocument();
  });

  it("does not show a bot badge on human rows", () => {
    const events: ClickEvent[] = [
      { id: 1, ts: 1700000000, country: "US", referer: null, city: null, bot: false },
    ];
    render(withProviders(<RecentEventsTable events={events} />, { withRouter: false }));

    expect(screen.queryByText("Bot")).not.toBeInTheDocument();
  });
});
