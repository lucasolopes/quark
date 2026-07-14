import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Login } from "./Login";
import { withProviders } from "@/test-utils";

describe("Login", () => {
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

  it("valid token is stored and the probe request is made", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ links: [], next_after: null }), { status: 200 }),
    );
    render(withProviders(<Login />));
    await userEvent.type(screen.getByLabelText(/token/i), "secret");
    await userEvent.click(screen.getByRole("button", { name: /sign in/i }));
    expect(localStorage.getItem("quark_admin_token")).toBe("secret");
  });

  it("invalid token (401) shows a specific error", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 401 }));
    render(withProviders(<Login />));
    await userEvent.type(screen.getByLabelText(/token/i), "wrong");
    await userEvent.click(screen.getByRole("button", { name: /sign in/i }));
    expect(await screen.findByText(/invalid token/i)).toBeInTheDocument();
  });

  it("network/5xx error shows a generic connection message, not 'invalid token'", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 503 }));
    render(withProviders(<Login />));
    await userEvent.type(screen.getByLabelText(/token/i), "secret");
    await userEvent.click(screen.getByRole("button", { name: /sign in/i }));
    expect(await screen.findByText(/could not connect/i)).toBeInTheDocument();
    expect(screen.queryByText(/invalid token/i)).not.toBeInTheDocument();
  });
});
