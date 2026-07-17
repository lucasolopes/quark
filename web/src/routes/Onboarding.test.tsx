import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Onboarding } from "./Onboarding";
import { withProviders } from "@/test-utils";
import type { Membership } from "@/lib/types";

const memberships: Membership[] = [
  { tenant_id: 1, name: "Acme", slug: "acme", role: "Owner" },
];

describe("Onboarding", () => {
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

  it("shows an error when switching to a workspace fails", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 403 }));
    render(withProviders(<Onboarding memberships={memberships} />));
    await userEvent.click(screen.getByRole("button", { name: /acme/i }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/could not switch workspaces/i);
  });

  it("shows a rate-limit message when switching is throttled", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 429 }));
    render(withProviders(<Onboarding memberships={memberships} />));
    await userEvent.click(screen.getByRole("button", { name: /acme/i }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/too many requests/i);
  });
});
