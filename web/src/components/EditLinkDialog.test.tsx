import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { EditLinkDialog } from "./EditLinkDialog";
import { withProviders } from "@/test-utils";
import type { Link } from "@/lib/types";

function makeLink(overrides: Partial<Link> = {}): Link {
  return { id: 1, code: "6lB362J", url: "https://ok.com", expiry: null, created: 1700000000, ...overrides };
}

function patchBody(fetchMock: ReturnType<typeof vi.spyOn>): Record<string, unknown> {
  return JSON.parse((fetchMock.mock.calls[0][1] as RequestInit).body as string);
}

describe("EditLinkDialog", () => {
  beforeEach(() => { localStorage.setItem("quark_admin_token", "s"); vi.restoreAllMocks(); });

  it("sends app_ios: null when a pre-filled iOS destination is cleared", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 200 }));
    const link = makeLink({ app_ios: "https://apps.apple.com/app/x" });
    render(withProviders(<EditLinkDialog link={link} open onOpenChange={() => {}} />, { withRouter: false }));

    await userEvent.clear(screen.getByLabelText(/ios/i));
    await userEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => expect(fetchMock).toHaveBeenCalledOnce());
    const body = patchBody(fetchMock);
    expect(body).toHaveProperty("app_ios", null);
    // Android was never set and stays empty → omitted, not null.
    expect(body).not.toHaveProperty("app_android");
  });

  it("omits app fields that were empty and stay empty", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 200 }));
    const link = makeLink();
    render(withProviders(<EditLinkDialog link={link} open onOpenChange={() => {}} />, { withRouter: false }));

    await userEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => expect(fetchMock).toHaveBeenCalledOnce());
    const body = patchBody(fetchMock);
    expect(body).not.toHaveProperty("app_ios");
    expect(body).not.toHaveProperty("app_android");
  });

  it("sends the trimmed value when an app destination is set", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 200 }));
    const link = makeLink();
    render(withProviders(<EditLinkDialog link={link} open onOpenChange={() => {}} />, { withRouter: false }));

    await userEvent.type(screen.getByLabelText(/ios/i), "https://apps.apple.com/app/y");
    await userEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => expect(fetchMock).toHaveBeenCalledOnce());
    const body = patchBody(fetchMock);
    expect(body).toHaveProperty("app_ios", "https://apps.apple.com/app/y");
  });
});
