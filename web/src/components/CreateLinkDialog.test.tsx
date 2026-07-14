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

  it("adding a redirect rule sends it in the create request", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ code: "6lB362J", url: "https://ok.com" }), { status: 200 }),
    );
    render(withProviders(<CreateLinkDialog open onOpenChange={() => {}} />, { withRouter: false }));
    await userEvent.type(screen.getByLabelText(/url/i), "https://ok.com");

    await userEvent.click(screen.getByText(/redirect rules/i));
    await userEvent.click(screen.getByRole("button", { name: /add rule/i }));
    await userEvent.selectOptions(screen.getByLabelText(/match on/i), "country");
    await userEvent.type(screen.getByLabelText(/values/i), "BR, PT");
    await userEvent.type(screen.getByLabelText(/destination url/i), "https://ok.com/br");

    await userEvent.click(screen.getByRole("button", { name: /^create link$/i }));

    expect(fetchMock).toHaveBeenCalledOnce();
    const body = JSON.parse(String(fetchMock.mock.calls[0][1]?.body));
    expect(body.rules).toEqual([{ field: "country", values: ["BR", "PT"], to: "https://ok.com/br" }]);
  });
});
