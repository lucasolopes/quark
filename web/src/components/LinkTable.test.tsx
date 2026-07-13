import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { LinkTable } from "./LinkTable";
import type { Link } from "@/lib/types";

function wrap(ui: React.ReactNode) {
  return <MemoryRouter>{ui}</MemoryRouter>;
}

const link: Link = {
  id: 1,
  code: "6lB362J",
  url: "https://exemplo.com/pagina-bem-longa",
  expiry: null,
  created: 1700000000,
};

describe("LinkTable — QR code", () => {
  it("abre o dialog de QR code com a URL curta e o botão de baixar", async () => {
    render(wrap(<LinkTable links={[link]} onEdit={() => {}} onDelete={() => {}} />));

    await userEvent.click(screen.getByRole("button", { name: /mais ações para 6lB362J/i }));
    await userEvent.click(await screen.findByRole("menuitem", { name: /qr code/i }));

    expect(await screen.findByRole("dialog", { name: /qr code de 6lB362J/i })).toBeInTheDocument();
    expect(screen.getByText(`${window.location.origin}/6lB362J`)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /baixar png/i })).toBeInTheDocument();
  });

  it("fecha o dialog de QR code ao cancelar", async () => {
    render(wrap(<LinkTable links={[link]} onEdit={() => {}} onDelete={() => {}} />));

    await userEvent.click(screen.getByRole("button", { name: /mais ações para 6lB362J/i }));
    await userEvent.click(await screen.findByRole("menuitem", { name: /qr code/i }));
    await screen.findByRole("dialog", { name: /qr code de 6lB362J/i });

    await userEvent.click(screen.getByRole("button", { name: /cancelar/i }));
    expect(screen.queryByRole("dialog", { name: /qr code de 6lB362J/i })).not.toBeInTheDocument();
  });
});
