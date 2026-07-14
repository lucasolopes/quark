import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { Blocklist } from "./Blocklist";
import { withProviders } from "@/test-utils";

describe("Blocklist", () => {
  beforeEach(() => { localStorage.setItem("quark_admin_token", "s"); vi.restoreAllMocks(); });

  it("lists the domains", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({ domains: ["evil.com"] }), { status: 200 }));
    render(withProviders(<Blocklist />, { withRouter: false }));
    expect(await screen.findByText("evil.com")).toBeInTheDocument();
  });

  it("empty state", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({ domains: [] }), { status: 200 }));
    render(withProviders(<Blocklist />, { withRouter: false }));
    expect(await screen.findByText(/no blocked domains/i)).toBeInTheDocument();
  });
});
