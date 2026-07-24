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

  it("create flow always sends kind: generic (no type selector) and reveals the secret once", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation((_input, init) => {
      const method = init?.method ?? "GET";
      if (method === "POST") return Promise.resolve(jsonResponse({ id: 2, secret: "whsec_rawsecret123" }));
      return Promise.resolve(jsonResponse({ webhooks: [] }));
    });

    render(withProviders(<Webhooks />, { withRouter: false }));
    await screen.findByText(/no webhooks yet/i);

    await userEvent.click(screen.getAllByRole("button", { name: /add webhook/i })[0]);
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).queryByLabelText(/^type$/i)).not.toBeInTheDocument();
    expect(within(dialog).getByText(/a signing secret will be generated/i)).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText(/endpoint url/i), "https://sink.example.com/hook");
    await userEvent.click(screen.getByRole("checkbox", { name: /link created/i }));

    await userEvent.click(within(dialog).getByRole("button", { name: /add webhook/i }));

    expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining("/admin/webhooks"),
      expect.objectContaining({ method: "POST" }),
    );
    const postCall = fetchMock.mock.calls.find(([, init]) => init?.method === "POST");
    const requestInit = postCall?.[1];
    if (!requestInit) throw new Error("expected a POST call with an init");
    expect(JSON.parse(requestInit.body as string)).toMatchObject({ kind: "generic" });
    expect(await screen.findByDisplayValue("whsec_rawsecret123")).toBeInTheDocument();
    expect(screen.getByText(/won't be shown again/i)).toBeInTheDocument();
  });

  it("shows the kind badge for webhooks created via the API with a non-generic kind", async () => {
    const slackWebhook = { ...SAMPLE_WEBHOOK, id: 4, kind: "slack" };
    vi.spyOn(globalThis, "fetch").mockResolvedValue(jsonResponse({ webhooks: [slackWebhook] }));
    render(withProviders(<Webhooks />, { withRouter: false }));
    await screen.findByText("https://example.com/hooks/quark");
    expect(screen.getByText(/^slack$/i)).toBeInTheDocument();
  });

  it("renders each webhook as a card with a status dot colored per health state", async () => {
    const active = { ...SAMPLE_WEBHOOK, id: 10, url: "https://active.example.com/hook" };
    const failing = {
      ...SAMPLE_WEBHOOK,
      id: 11,
      url: "https://failing.example.com/hook",
      last_delivery_status: { state: "error", detail: "connection refused" },
    };
    const paused = { ...SAMPLE_WEBHOOK, id: 12, url: "https://paused.example.com/hook", active: false };
    vi.spyOn(globalThis, "fetch").mockResolvedValue(jsonResponse({ webhooks: [active, failing, paused] }));

    render(withProviders(<Webhooks />, { withRouter: false }));

    const cards = await screen.findAllByTestId("webhook-card");
    expect(cards).toHaveLength(3);
    // Semantic list: each card is a real <li> so the stack reads as a list.
    expect(screen.getAllByRole("listitem")).toHaveLength(3);

    const activeCard = cards.find((c) => within(c).queryByText("https://active.example.com/hook"));
    const failingCard = cards.find((c) => within(c).queryByText("https://failing.example.com/hook"));
    const pausedCard = cards.find((c) => within(c).queryByText("https://paused.example.com/hook"));
    if (!activeCard || !failingCard || !pausedCard) throw new Error("expected one card per webhook");

    // active/healthy = bg-primary, failing = bg-destructive, paused = bg-muted-foreground.
    expect(within(activeCard).getByTestId("webhook-status-dot")).toHaveClass("bg-primary");
    expect(within(failingCard).getByTestId("webhook-status-dot")).toHaveClass("bg-destructive");
    expect(within(failingCard).getByText(/connection refused/i)).toBeInTheDocument();
    expect(within(pausedCard).getByTestId("webhook-status-dot")).toHaveClass("bg-muted-foreground");
  });

  it("toggles the active state via the switch and calls the API", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation((_input, init) => {
      const method = init?.method ?? "GET";
      if (method === "PATCH") return Promise.resolve(jsonResponse({ ...SAMPLE_WEBHOOK, active: false }));
      return Promise.resolve(jsonResponse({ webhooks: [SAMPLE_WEBHOOK] }));
    });

    render(withProviders(<Webhooks />, { withRouter: false }));
    await screen.findByText("https://example.com/hooks/quark");

    await userEvent.click(screen.getByRole("switch", { name: /deactivate webhook/i }));

    expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining("/admin/webhooks/1"),
      expect.objectContaining({ method: "PATCH" }),
    );
    const patchCall = fetchMock.mock.calls.find(([, init]) => init?.method === "PATCH");
    const requestInit = patchCall?.[1];
    if (!requestInit) throw new Error("expected a PATCH call with an init");
    expect(JSON.parse(requestInit.body as string)).toMatchObject({ active: false });
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
