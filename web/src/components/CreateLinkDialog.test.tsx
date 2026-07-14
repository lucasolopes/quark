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

  it("adding 2 variants sends the variants array with numeric weights", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ code: "6lB362J", url: "https://ok.com" }), { status: 200 }),
    );
    render(withProviders(<CreateLinkDialog open onOpenChange={() => {}} />, { withRouter: false }));

    await userEvent.type(screen.getByLabelText(/^url$/i), "https://ok.com");
    await userEvent.click(screen.getByRole("button", { name: /a\/b variants/i }));
    await userEvent.click(screen.getByRole("button", { name: /add variant/i }));
    await userEvent.click(screen.getByRole("button", { name: /add variant/i }));

    const urlInputs = screen.getAllByLabelText(/variant url/i);
    const weightInputs = screen.getAllByLabelText(/weight/i);
    await userEvent.type(urlInputs[0], "https://variant-a.com");
    await userEvent.clear(weightInputs[0]);
    await userEvent.type(weightInputs[0], "3");
    await userEvent.type(urlInputs[1], "https://variant-b.com");

    await userEvent.click(screen.getByRole("button", { name: /^create link$/i }));

    expect(fetchMock).toHaveBeenCalledOnce();
    const [, init] = fetchMock.mock.calls[0];
    const body = JSON.parse((init as RequestInit).body as string);
    expect(body.variants).toEqual([
      { url: "https://variant-a.com", weight: 3 },
      { url: "https://variant-b.com", weight: 1 },
    ]);
  });
});
