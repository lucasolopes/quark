import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { CreateLinkDialog } from "./CreateLinkDialog";
import { withProviders } from "@/test-utils";

describe("CreateLinkDialog", () => {
  beforeEach(() => { localStorage.setItem("quark_admin_token", "s"); vi.restoreAllMocks(); });

  it("rejects a non-http(s) URL without calling the API", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch");
    render(withProviders(<CreateLinkDialog open onOpenChange={() => {}} />, { withRouter: false }));
    await userEvent.type(screen.getByLabelText(/url/i), "ftp://nope");
    await userEvent.click(screen.getByRole("button", { name: /create/i }));
    expect(await screen.findByText(/invalid url|http/i)).toBeInTheDocument();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("valid URL calls the API", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ code: "6lB362J", url: "https://ok.com" }), { status: 200 }),
    );
    render(withProviders(<CreateLinkDialog open onOpenChange={() => {}} />, { withRouter: false }));
    await userEvent.type(screen.getByLabelText(/url/i), "https://ok.com");
    await userEvent.click(screen.getByRole("button", { name: /create/i }));
    expect(fetchMock).toHaveBeenCalledOnce();
  });

  it("renders the app destination inputs and submits the iOS value", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ code: "6lB362J", url: "https://ok.com" }), { status: 200 }),
    );
    render(withProviders(<CreateLinkDialog open onOpenChange={() => {}} />, { withRouter: false }));
    const iosInput = screen.getByLabelText(/ios/i);
    const androidInput = screen.getByLabelText(/android/i);
    expect(iosInput).toBeInTheDocument();
    expect(androidInput).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText(/url/i), "https://ok.com");
    await userEvent.type(iosInput, "https://apps.apple.com/app/x");
    await userEvent.click(screen.getByRole("button", { name: /create/i }));
    expect(fetchMock).toHaveBeenCalledOnce();
    const body = JSON.parse((fetchMock.mock.calls[0][1] as RequestInit).body as string);
    expect(body.app_ios).toBe("https://apps.apple.com/app/x");
    expect(body.app_android).toBeUndefined();
  });
});
