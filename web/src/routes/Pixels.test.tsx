import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Pixels } from "./Pixels";
import { withProviders } from "@/test-utils";

const SAMPLE_PIXEL = {
  id: 7,
  provider: "ga4",
  credentials: { measurement_id: "G-ABC123", api_secret: "••••" },
  active: true,
  created: 1700000000,
};

describe("Pixels", () => {
  beforeEach(() => { localStorage.setItem("quark_admin_token", "s"); vi.restoreAllMocks(); });

  it("creating a GA4 pixel sends the provider and its credentials", async () => {
    const createdPixel = {
      id: 1,
      provider: "ga4",
      credentials: { measurement_id: "G-ABC123", api_secret: "••••" },
      active: true,
      created: 1700000000,
    };
    const fetchMock = vi.spyOn(globalThis, "fetch")
      .mockResolvedValueOnce(new Response(JSON.stringify({ pixels: [] }), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(createdPixel), { status: 201 }))
      // refetch of `["pixels"]` triggered by the create mutation's invalidation
      .mockResolvedValue(new Response(JSON.stringify({ pixels: [createdPixel] }), { status: 200 }));

    render(withProviders(<Pixels />, { withRouter: false }));

    await userEvent.click(await screen.findByRole("button", { name: /add pixel/i }));
    await userEvent.type(screen.getByLabelText(/measurement id/i), "G-ABC123");
    await userEvent.type(screen.getByLabelText(/api secret/i), "s3cr3t");
    const submitButtons = screen.getAllByRole("button", { name: /^add pixel$/i });
    await userEvent.click(submitButtons[submitButtons.length - 1]);

    const createCall = await vi.waitFor(() => {
      const call = fetchMock.mock.calls.find(([, opts]) => (opts as RequestInit | undefined)?.method === "POST");
      if (!call) throw new Error("POST /admin/pixels not called yet");
      return call;
    });
    expect(String(createCall[0])).toMatch(/\/admin\/pixels$/);
    const body = JSON.parse(String((createCall[1] as RequestInit).body));
    expect(body).toEqual({
      provider: "ga4",
      credentials: { measurement_id: "G-ABC123", api_secret: "s3cr3t" },
    });
  });

  it("lists pixels with masked credentials, never the raw value", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(
        JSON.stringify({
          pixels: [
            {
              id: 1,
              provider: "ga4",
              credentials: { measurement_id: "G-ABC123", api_secret: "••••" },
              active: true,
              created: 1700000000,
            },
          ],
        }),
        { status: 200 },
      ),
    );

    render(withProviders(<Pixels />, { withRouter: false }));

    expect(await screen.findByText(/G-ABC123/)).toBeInTheDocument();
    expect(await screen.findByText(/••••/)).toBeInTheDocument();
    expect(screen.queryByText("s3cr3t")).not.toBeInTheDocument();
  });

  it("empty state", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify({ pixels: [] }), { status: 200 }));
    render(withProviders(<Pixels />, { withRouter: false }));
    expect(await screen.findByText(/no pixels configured/i)).toBeInTheDocument();
  });

  it("renders each pixel as a card with its provider and active badges", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ pixels: [SAMPLE_PIXEL] }), { status: 200 }),
    );

    render(withProviders(<Pixels />, { withRouter: false }));

    const cards = await screen.findAllByTestId("pixel-card");
    expect(cards).toHaveLength(1);
    expect(screen.getAllByRole("listitem")).toHaveLength(1);
    expect(within(cards[0]).getByText("G-ABC123")).toBeInTheDocument();
    expect(within(cards[0]).getByText(/^ga4$/i)).toBeInTheDocument();
    expect(within(cards[0]).getByText(/^active$/i)).toBeInTheDocument();
  });

  it("remove asks for confirmation and calls the delete endpoint", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation((_input, init) => {
      const method = init?.method ?? "GET";
      if (method === "DELETE") return Promise.resolve(new Response(null, { status: 204 }));
      return Promise.resolve(new Response(JSON.stringify({ pixels: [SAMPLE_PIXEL] }), { status: 200 }));
    });

    render(withProviders(<Pixels />, { withRouter: false }));
    await screen.findByText("G-ABC123");

    await userEvent.click(screen.getByRole("button", { name: /remove this pixel/i }));
    const dialog = screen.getByRole("alertdialog");
    await userEvent.click(within(dialog).getByRole("button", { name: /^remove$/i }));

    expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining("/admin/pixels/7"),
      expect.objectContaining({ method: "DELETE" }),
    );
  });
});
