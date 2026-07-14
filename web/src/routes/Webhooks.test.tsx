import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Webhooks } from "./Webhooks";
import { withProviders } from "@/test-utils";

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status });
}

const SAMPLE_WEBHOOK = {
  id: 1,
  url: "https://example.com/hooks/quark",
  events: ["link.created", "link.clicked"],
  active: true,
  created: 1700000000,
  kind: "generic",
  secret_masked: "whsec_••••",
};

describe("Webhooks", () => {
  beforeEach(() => { localStorage.setItem("quark_admin_token", "s"); vi.restoreAllMocks(); });

  it("lists the webhooks", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(jsonResponse({ webhooks: [SAMPLE_WEBHOOK] }));
    render(withProviders(<Webhooks />, { withRouter: false }));
    expect(await screen.findByText("https://example.com/hooks/quark")).toBeInTheDocument();
    expect(screen.getByText(/link created/i)).toBeInTheDocument();
    expect(screen.getByText(/link clicked/i)).toBeInTheDocument();
  });

  it("empty state", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(jsonResponse({ webhooks: [] }));
    render(withProviders(<Webhooks />, { withRouter: false }));
    expect(await screen.findByText(/no webhooks yet/i)).toBeInTheDocument();
  });

  it("create flow calls the API and reveals the secret once (generic)", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation((_input, init) => {
      const method = init?.method ?? "GET";
      if (method === "POST") return Promise.resolve(jsonResponse({ id: 2, secret: "whsec_rawsecret123" }));
      return Promise.resolve(jsonResponse({ webhooks: [] }));
    });

    render(withProviders(<Webhooks />, { withRouter: false }));
    await screen.findByText(/no webhooks yet/i);

    await userEvent.click(screen.getAllByRole("button", { name: /add webhook/i })[0]);
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText(/a signing secret will be generated/i)).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText(/endpoint url/i), "https://sink.example.com/hook");
    await userEvent.click(screen.getByRole("checkbox", { name: /link created/i }));

    await userEvent.click(within(dialog).getByRole("button", { name: /add webhook/i }));

    expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining("/admin/webhooks"),
      expect.objectContaining({ method: "POST" }),
    );
    const [, requestInit] = fetchMock.mock.calls.find(([, init]) => init?.method === "POST")!;
    expect(JSON.parse(requestInit!.body as string)).toMatchObject({ kind: "generic" });
    expect(await screen.findByDisplayValue("whsec_rawsecret123")).toBeInTheDocument();
    expect(screen.getByText(/won't be shown again/i)).toBeInTheDocument();
  });

  it("selecting Slack sends kind: slack, adapts the URL field and hides the secret notice", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation((_input, init) => {
      const method = init?.method ?? "GET";
      if (method === "POST") return Promise.resolve(jsonResponse({ id: 3, secret: "" }));
      return Promise.resolve(jsonResponse({ webhooks: [] }));
    });

    render(withProviders(<Webhooks />, { withRouter: false }));
    await screen.findByText(/no webhooks yet/i);

    await userEvent.click(screen.getAllByRole("button", { name: /add webhook/i })[0]);
    const dialog = screen.getByRole("dialog");

    await userEvent.selectOptions(within(dialog).getByLabelText(/^type$/i), "slack");

    expect(within(dialog).getByLabelText(/slack incoming webhook url/i)).toBeInTheDocument();
    expect(within(dialog).queryByText(/a signing secret will be generated/i)).not.toBeInTheDocument();
    expect(within(dialog).getByText(/no signing secret/i)).toBeInTheDocument();

    await userEvent.type(within(dialog).getByLabelText(/slack incoming webhook url/i), "https://hooks.slack.com/services/x");
    await userEvent.click(within(dialog).getByRole("checkbox", { name: /link created/i }));
    await userEvent.click(within(dialog).getByRole("button", { name: /add webhook/i }));

    const [, requestInit] = fetchMock.mock.calls.find(([, init]) => init?.method === "POST")!;
    expect(JSON.parse(requestInit!.body as string)).toMatchObject({ kind: "slack" });

    expect(screen.queryByText(/won't be shown again/i)).not.toBeInTheDocument();
  });

  it("rejects submitting with no event selected", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(jsonResponse({ webhooks: [] }));
    render(withProviders(<Webhooks />, { withRouter: false }));
    await screen.findByText(/no webhooks yet/i);

    await userEvent.click(screen.getAllByRole("button", { name: /add webhook/i })[0]);
    await userEvent.type(screen.getByLabelText(/endpoint url/i), "https://sink.example.com/hook");
    const dialog = screen.getByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: /add webhook/i }));

    expect(await screen.findByText(/choose at least one event/i)).toBeInTheDocument();
  });

  it("delete confirms and calls the API", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation((_input, init) => {
      const method = init?.method ?? "GET";
      if (method === "DELETE") return Promise.resolve(new Response(null, { status: 204 }));
      return Promise.resolve(jsonResponse({ webhooks: [SAMPLE_WEBHOOK] }));
    });

    render(withProviders(<Webhooks />, { withRouter: false }));
    await screen.findByText("https://example.com/hooks/quark");

    await userEvent.click(screen.getByRole("button", { name: /delete webhook/i }));
    const dialog = screen.getByRole("alertdialog");
    await userEvent.click(within(dialog).getByRole("button", { name: /delete/i }));

    expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining("/admin/webhooks/1"),
      expect.objectContaining({ method: "DELETE" }),
    );
  });
});
