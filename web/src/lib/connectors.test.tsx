import { describe, it, expect, afterEach, vi } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { useConnectedIds } from "./connectors";

function wrapper({ children }: { children: ReactNode }) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
}

describe("useConnectedIds", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("distinguishes generic webhooks by connector_id instead of lighting up all generic connectors together", async () => {
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
      const url = String(typeof input === "string" ? input : (input as Request).url ?? input);
      if (url.includes("/admin/webhooks")) {
        return new Response(
          JSON.stringify({
            webhooks: [
              { id: 1, url: "https://hooks.zapier.com/x", events: [], active: true, created: 1, kind: "generic", connector_id: "zapier", secret_masked: "" },
              { id: 2, url: "https://hook.make.com/y", events: [], active: true, created: 2, kind: "generic", connector_id: "make", secret_masked: "" },
            ],
          }),
          { status: 200 },
        );
      }
      if (url.includes("/admin/pixels")) return new Response(JSON.stringify({ pixels: [] }), { status: 200 });
      if (url.includes("/admin/integrations/sheets/status")) return new Response("", { status: 404 });
      return new Response("", { status: 404, statusText: `unexpected ${url}` });
    });

    const { result } = renderHook(() => useConnectedIds(), { wrapper });

    await waitFor(() => expect(result.current.has("zapier")).toBe(true));
    expect(result.current.has("make")).toBe(true);
    expect(result.current.has("n8n")).toBe(false);
  });
});
