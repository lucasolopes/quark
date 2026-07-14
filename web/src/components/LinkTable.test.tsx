import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { LinkTable } from "./LinkTable";
import { withProviders } from "@/test-utils";
import type { Link } from "@/lib/types";

const link: Link = {
  id: 1,
  code: "6lB362J",
  url: "https://example.com/a-pretty-long-page",
  expiry: null,
  created: 1700000000,
  tags: [],
  visits: 0,
  rules: [],
};

describe("LinkTable — QR code", () => {
  it("opens the QR code dialog with the short URL and the download button", async () => {
    render(withProviders(<LinkTable links={[link]} onEdit={() => {}} onDelete={() => {}} />));

    await userEvent.click(screen.getByRole("button", { name: /more actions for 6lB362J/i }));
    await userEvent.click(await screen.findByRole("menuitem", { name: /qr code/i }));

    expect(await screen.findByRole("dialog", { name: /qr code for 6lB362J/i })).toBeInTheDocument();
    expect(screen.getByText(`${window.location.origin}/6lB362J`)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /download png/i })).toBeInTheDocument();
  });

  it("closes the QR code dialog on cancel", async () => {
    render(withProviders(<LinkTable links={[link]} onEdit={() => {}} onDelete={() => {}} />));

    await userEvent.click(screen.getByRole("button", { name: /more actions for 6lB362J/i }));
    await userEvent.click(await screen.findByRole("menuitem", { name: /qr code/i }));
    await screen.findByRole("dialog", { name: /qr code for 6lB362J/i });

    await userEvent.click(screen.getByRole("button", { name: /cancel/i }));
    expect(screen.queryByRole("dialog", { name: /qr code for 6lB362J/i })).not.toBeInTheDocument();
  });
});

describe("LinkTable — tags", () => {
  it("renders tags as badges", () => {
    const tagged: Link = { ...link, tags: ["promo", "summer"] };
    render(withProviders(<LinkTable links={[tagged]} onEdit={() => {}} onDelete={() => {}} />));

    expect(screen.getByText("promo")).toBeInTheDocument();
    expect(screen.getByText("summer")).toBeInTheDocument();
  });

  it("collapses extra tags into a +k badge", () => {
    const tagged: Link = { ...link, tags: ["a", "b", "c", "d", "e"] };
    render(withProviders(<LinkTable links={[tagged]} onEdit={() => {}} onDelete={() => {}} />));

    expect(screen.getByText("a")).toBeInTheDocument();
    expect(screen.getByText("b")).toBeInTheDocument();
    expect(screen.getByText("c")).toBeInTheDocument();
    expect(screen.queryByText("d")).not.toBeInTheDocument();
    expect(screen.getByText("+2")).toBeInTheDocument();
  });

  it("shows a dash when a link has no tags", () => {
    render(withProviders(<LinkTable links={[link]} onEdit={() => {}} onDelete={() => {}} />));
    expect(screen.getAllByText("—").length).toBeGreaterThan(0);
  });
});

describe("LinkTable — visits", () => {
  it("shows visits/max when max_visits is set", () => {
    const limited: Link = { ...link, visits: 12, max_visits: 100 };
    render(withProviders(<LinkTable links={[limited]} onEdit={() => {}} onDelete={() => {}} />));
    expect(screen.getByText("12 / 100")).toBeInTheDocument();
  });

  it("shows only the visit count when there is no max_visits", () => {
    const unlimited: Link = { ...link, visits: 7 };
    render(withProviders(<LinkTable links={[unlimited]} onEdit={() => {}} onDelete={() => {}} />));
    expect(screen.getByText("7")).toBeInTheDocument();
  });
});

describe("LinkTable — redirect rules badge", () => {
  it("shows a rule-count badge when the link has rules", () => {
    const linkWithRules: Link = {
      ...link,
      rules: [{ field: "country", values: ["BR"], to: "https://example.com/br" }],
    };
    render(withProviders(<LinkTable links={[linkWithRules]} onEdit={() => {}} onDelete={() => {}} />));
    expect(screen.getByText("1 rules")).toBeInTheDocument();
  });

  it("shows no badge when the link has no rules", () => {
    render(withProviders(<LinkTable links={[link]} onEdit={() => {}} onDelete={() => {}} />));
    expect(screen.queryByText(/rules?$/)).not.toBeInTheDocument();
  });
});
