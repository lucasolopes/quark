import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it } from "vitest";
import { PageHeader } from "./PageHeader";

describe("PageHeader", () => {
  it("renders title as h1, subtitle and actions", () => {
    render(
      <MemoryRouter>
        <PageHeader title="Links" subtitle="128 links" actions={<button>Novo</button>} back={{ label: "Voltar", to: "/links" }} />
      </MemoryRouter>,
    );
    expect(screen.getByRole("heading", { level: 1, name: "Links" })).toBeInTheDocument();
    expect(screen.getByText("128 links")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Novo" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Voltar" })).toHaveAttribute("href", "/links");
  });
});
