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
  variants: [],
};

describe("LinkTable — A/B variants badge", () => {
  it("shows a badge with the variant count when the link has variants", () => {
    const linkWithVariants: Link = {
      ...link,
      variants: [
        { url: "https://variant-a.com", weight: 1 },
        { url: "https://variant-b.com", weight: 1 },
      ],
    };
    render(withProviders(<LinkTable links={[linkWithVariants]} onEdit={() => {}} onDelete={() => {}} />));
    expect(screen.getByText("A/B: 2")).toBeInTheDocument();
  });

  it("shows no badge when the link has no variants", () => {
    render(withProviders(<LinkTable links={[link]} onEdit={() => {}} onDelete={() => {}} />));
    expect(screen.queryByText(/^A\/B:/)).not.toBeInTheDocument();
  });
});

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
