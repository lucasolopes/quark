import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { CreateLinkDialog } from "./CreateLinkDialog";
import { withProviders } from "@/test-utils";

describe("CreateLinkDialog", () => {
  beforeEach(() => {
    localStorage.setItem("quark_admin_token", "s");
    localStorage.removeItem("quark.utmTemplates");
    vi.restoreAllMocks();
  });

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

  it("sends parsed tags from the comma-separated field", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ code: "6lB362J", url: "https://ok.com" }), { status: 200 }),
    );
    render(withProviders(<CreateLinkDialog open onOpenChange={() => {}} />, { withRouter: false }));
    await userEvent.type(screen.getByLabelText(/url/i), "https://ok.com");
    await userEvent.type(screen.getByLabelText(/tags/i), "promo, summer ,  2026");
    await userEvent.click(screen.getByRole("button", { name: /create/i }));

    expect(fetchMock).toHaveBeenCalledOnce();
    const [, init] = fetchMock.mock.calls[0];
    const body = JSON.parse(String(init!.body)) as { tags?: string[] };
    expect(body.tags).toEqual(["promo", "summer", "2026"]);
  });

  it("sends max_visits as a number when set", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ code: "6lB362J", url: "https://ok.com" }), { status: 200 }),
    );
    render(withProviders(<CreateLinkDialog open onOpenChange={() => {}} />, { withRouter: false }));
    await userEvent.type(screen.getByLabelText(/url/i), "https://ok.com");
    await userEvent.type(screen.getByLabelText(/max visits/i), "100");
    await userEvent.click(screen.getByRole("button", { name: /create/i }));
    expect(fetchMock).toHaveBeenCalledOnce();
    const [, init] = fetchMock.mock.calls[0];
    const body = JSON.parse(String(init?.body));
    expect(body.max_visits).toBe(100);
  });

  it("omits max_visits (unlimited) when the field is left empty", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ code: "6lB362J", url: "https://ok.com" }), { status: 200 }),
    );
    render(withProviders(<CreateLinkDialog open onOpenChange={() => {}} />, { withRouter: false }));
    await userEvent.type(screen.getByLabelText(/url/i), "https://ok.com");
    await userEvent.click(screen.getByRole("button", { name: /create/i }));
    expect(fetchMock).toHaveBeenCalledOnce();
    const [, init] = fetchMock.mock.calls[0];
    const body = JSON.parse(String(init?.body));
    expect(body).not.toHaveProperty("max_visits");
  });

  it("filling in UTM fields sends the utm-tagged url on submit", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ code: "6lB362J", url: "https://ok.com" }), { status: 200 }),
    );
    render(withProviders(<CreateLinkDialog open onOpenChange={() => {}} />, { withRouter: false }));
    await userEvent.type(screen.getByLabelText(/^url$/i), "https://ok.com");
    await userEvent.click(screen.getByRole("button", { name: /utm parameters/i }));
    await userEvent.type(screen.getByLabelText(/source/i), "newsletter");
    await userEvent.type(screen.getByLabelText(/medium/i), "email");
    await userEvent.click(screen.getByRole("button", { name: /create/i }));

    expect(fetchMock).toHaveBeenCalledOnce();
    const [, init] = fetchMock.mock.calls[0];
    const body = JSON.parse((init as RequestInit).body as string) as { url: string };
    expect(body.url).toBe("https://ok.com/?utm_source=newsletter&utm_medium=email");
  });

  it("without any utm field filled, submits the plain url unchanged", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ code: "6lB362J", url: "https://ok.com" }), { status: 200 }),
    );
    render(withProviders(<CreateLinkDialog open onOpenChange={() => {}} />, { withRouter: false }));
    await userEvent.type(screen.getByLabelText(/^url$/i), "https://ok.com");
    await userEvent.click(screen.getByRole("button", { name: /create/i }));

    expect(fetchMock).toHaveBeenCalledOnce();
    const [, init] = fetchMock.mock.calls[0];
    const body = JSON.parse((init as RequestInit).body as string) as { url: string };
    expect(body.url).toBe("https://ok.com");
  });

  it("applying a saved template fills the utm fields", async () => {
    localStorage.setItem(
      "quark.utmTemplates",
      JSON.stringify({ "Spring launch": { source: "twitter", medium: "social" } }),
    );
    render(withProviders(<CreateLinkDialog open onOpenChange={() => {}} />, { withRouter: false }));
    await userEvent.click(screen.getByRole("button", { name: /utm parameters/i }));
    await userEvent.click(screen.getByRole("button", { name: /templates/i }));
    await userEvent.click(await screen.findByText("Spring launch"));

    expect(screen.getByLabelText(/source/i)).toHaveValue("twitter");
    expect(screen.getByLabelText(/medium/i)).toHaveValue("social");
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
