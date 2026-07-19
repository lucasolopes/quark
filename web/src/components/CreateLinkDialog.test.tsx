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
    await userEvent.click(screen.getByRole("button", { name: /^create link$/i }));
    expect(await screen.findByText(/invalid url|http/i)).toBeInTheDocument();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("valid URL calls the API", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ code: "6lB362J", url: "https://ok.com" }), { status: 200 }),
    );
    render(withProviders(<CreateLinkDialog open onOpenChange={() => {}} />, { withRouter: false }));
    await userEvent.type(screen.getByLabelText(/url/i), "https://ok.com");
    await userEvent.click(screen.getByRole("button", { name: /^create link$/i }));
    expect(fetchMock).toHaveBeenCalledOnce();
  });

  it("creates tags via the create button and sends them as an array", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ code: "6lB362J", url: "https://ok.com" }), { status: 200 }),
    );
    render(withProviders(<CreateLinkDialog open onOpenChange={() => {}} />, { withRouter: false }));
    await userEvent.type(screen.getByLabelText(/url/i), "https://ok.com");
    // Each tag: click "Create new tag", type the name, confirm with Enter.
    for (const name of ["promo", "summer", "2026"]) {
      await userEvent.click(screen.getByRole("button", { name: /create new tag/i }));
      await userEvent.type(screen.getByLabelText(/create new tag/i), `${name}{Enter}`);
    }
    await userEvent.click(screen.getByRole("button", { name: /^create link$/i }));

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
    await userEvent.click(screen.getByRole("button", { name: /scheduling and limits/i }));
    await userEvent.type(screen.getByLabelText(/max visits/i), "100");
    await userEvent.click(screen.getByRole("button", { name: /^create link$/i }));
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
    await userEvent.click(screen.getByRole("button", { name: /^create link$/i }));
    expect(fetchMock).toHaveBeenCalledOnce();
    const [, init] = fetchMock.mock.calls[0];
    const body = JSON.parse(String(init?.body));
    expect(body).not.toHaveProperty("max_visits");
  });

  it("sends fallback_url when the field is set", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ code: "6lB362J", url: "https://ok.com" }), { status: 200 }),
    );
    render(withProviders(<CreateLinkDialog open onOpenChange={() => {}} />, { withRouter: false }));
    await userEvent.type(screen.getByLabelText(/url/i), "https://ok.com");
    await userEvent.click(screen.getByRole("button", { name: /scheduling and limits/i }));
    await userEvent.type(screen.getByLabelText(/fallback/i), "https://ended.com");
    await userEvent.click(screen.getByRole("button", { name: /^create link$/i }));
    expect(fetchMock).toHaveBeenCalledOnce();
    const [, init] = fetchMock.mock.calls[0];
    const body = JSON.parse(String(init?.body));
    expect(body.fallback_url).toBe("https://ended.com");
  });

  it("sends password when the field is set", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ code: "6lB362J", url: "https://ok.com" }), { status: 200 }),
    );
    render(withProviders(<CreateLinkDialog open onOpenChange={() => {}} />, { withRouter: false }));
    await userEvent.type(screen.getByLabelText(/url/i), "https://ok.com");
    await userEvent.click(screen.getByRole("button", { name: /^password$/i }));
    await userEvent.type(screen.getByLabelText(/password/i), "hunter2");
    await userEvent.click(screen.getByRole("button", { name: /^create link$/i }));
    expect(fetchMock).toHaveBeenCalledOnce();
    const [, init] = fetchMock.mock.calls[0];
    const body = JSON.parse(String(init?.body));
    expect(body.password).toBe("hunter2");
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
    await userEvent.click(screen.getByRole("button", { name: /^create link$/i }));

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
    await userEvent.click(screen.getByRole("button", { name: /^create link$/i }));

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
    // The country values are picked from the searchable list (select-only).
    await userEvent.type(screen.getByLabelText(/values/i), "brazil");
    await userEvent.click(screen.getByRole("option", { name: /Brazil/i }));
    await userEvent.clear(screen.getByLabelText(/values/i));
    await userEvent.type(screen.getByLabelText(/values/i), "portugal");
    await userEvent.click(screen.getByRole("option", { name: /Portugal/i }));
    await userEvent.type(screen.getByLabelText(/destination url/i), "https://ok.com/br");

    await userEvent.click(screen.getByRole("button", { name: /^create link$/i }));

    expect(fetchMock).toHaveBeenCalledOnce();
    const body = JSON.parse(String(fetchMock.mock.calls[0][1]?.body));
    expect(body.rules).toEqual([{ field: "country", values: ["BR", "PT"], to: "https://ok.com/br" }]);
  });

  it("adding 2 variants defaults to an even 50/50 split and sends it as percentages", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ code: "6lB362J", url: "https://ok.com" }), { status: 200 }),
    );
    render(withProviders(<CreateLinkDialog open onOpenChange={() => {}} />, { withRouter: false }));
    await userEvent.type(screen.getByLabelText(/^url$/i), "https://ok.com");
    await userEvent.click(screen.getByRole("button", { name: /a\/b variants/i }));
    await userEvent.click(screen.getByRole("button", { name: /add variant/i }));
    await userEvent.click(screen.getByRole("button", { name: /add variant/i }));

    const urlInputs = screen.getAllByLabelText(/variant url/i);
    const weightInputs = screen.getAllByLabelText(/traffic percentage/i);
    // Two variants default to 50/50 without any manual entry.
    expect((weightInputs[0] as HTMLInputElement).value).toBe("50");
    expect((weightInputs[1] as HTMLInputElement).value).toBe("50");
    await userEvent.type(urlInputs[0], "https://variant-a.com");
    await userEvent.type(urlInputs[1], "https://variant-b.com");

    await userEvent.click(screen.getByRole("button", { name: /^create link$/i }));

    expect(fetchMock).toHaveBeenCalledOnce();
    const [, init] = fetchMock.mock.calls[0];
    const body = JSON.parse((init as RequestInit).body as string);
    expect(body.variants).toEqual([
      { url: "https://variant-a.com", weight: 50 },
      { url: "https://variant-b.com", weight: 50 },
    ]);
  });

  it("blocks submit when the variant percentages do not add up to 100", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ code: "6lB362J", url: "https://ok.com" }), { status: 200 }),
    );
    render(withProviders(<CreateLinkDialog open onOpenChange={() => {}} />, { withRouter: false }));
    await userEvent.type(screen.getByLabelText(/^url$/i), "https://ok.com");
    await userEvent.click(screen.getByRole("button", { name: /a\/b variants/i }));
    await userEvent.click(screen.getByRole("button", { name: /add variant/i }));
    await userEvent.click(screen.getByRole("button", { name: /add variant/i }));

    const urlInputs = screen.getAllByLabelText(/variant url/i);
    const weightInputs = screen.getAllByLabelText(/traffic percentage/i);
    await userEvent.type(urlInputs[0], "https://variant-a.com");
    await userEvent.type(urlInputs[1], "https://variant-b.com");
    // Break the 50/50 default so the total is 70, not 100.
    await userEvent.clear(weightInputs[0]);
    await userEvent.type(weightInputs[0], "20");

    await userEvent.click(screen.getByRole("button", { name: /^create link$/i }));

    expect(fetchMock).not.toHaveBeenCalled();
    expect(screen.getByRole("alert")).toHaveTextContent(/100%/);
  });

  it("converts the expiration value and unit into seconds", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ code: "6lB362J", url: "https://ok.com" }), { status: 200 }),
    );
    render(withProviders(<CreateLinkDialog open onOpenChange={() => {}} />, { withRouter: false }));
    await userEvent.type(screen.getByLabelText(/^url$/i), "https://ok.com");
    await userEvent.click(screen.getByRole("button", { name: /scheduling and limits/i }));
    await userEvent.type(screen.getByLabelText(/expires in/i), "2");
    await userEvent.selectOptions(screen.getByLabelText(/time unit/i), "hours");

    await userEvent.click(screen.getByRole("button", { name: /^create link$/i }));

    expect(fetchMock).toHaveBeenCalledOnce();
    const [, init] = fetchMock.mock.calls[0];
    const body = JSON.parse((init as RequestInit).body as string);
    expect(body.ttl).toBe(7200);
  });

  it("renders the app destination inputs and submits the iOS value", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ code: "6lB362J", url: "https://ok.com" }), { status: 200 }),
    );
    render(withProviders(<CreateLinkDialog open onOpenChange={() => {}} />, { withRouter: false }));
    await userEvent.click(screen.getByRole("button", { name: /app redirect/i }));
    const iosInput = screen.getByLabelText(/ios/i);
    const androidInput = screen.getByLabelText(/android/i);
    expect(iosInput).toBeInTheDocument();
    expect(androidInput).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText(/url/i), "https://ok.com");
    await userEvent.type(iosInput, "https://apps.apple.com/app/x");
    await userEvent.click(screen.getByRole("button", { name: /^create link$/i }));
    expect(fetchMock).toHaveBeenCalledOnce();
    const body = JSON.parse((fetchMock.mock.calls[0][1] as RequestInit).body as string);
    expect(body.app_ios).toBe("https://apps.apple.com/app/x");
    expect(body.app_android).toBeUndefined();
  });
});
