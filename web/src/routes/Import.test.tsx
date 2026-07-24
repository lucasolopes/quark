import { describe, it, expect, beforeEach, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Import } from "./Import";
import { withProviders } from "@/test-utils";

describe("Import", () => {
  beforeEach(() => { localStorage.setItem("quark_admin_token", "s"); vi.restoreAllMocks(); });

  it("renders the page header and the expected-format terminal with the real CSV sample", () => {
    render(withProviders(<Import />, { withRouter: false }));

    expect(screen.getByRole("heading", { level: 1, name: /import links/i })).toBeInTheDocument();
    expect(screen.getByText("import.csv")).toBeInTheDocument();
    expect(screen.getByText(/url,alias,ttl/)).toBeInTheDocument();
    expect(screen.getByText(/some\/long\/landing\/page/)).toBeInTheDocument();
  });

  it("uploads a CSV file (dashed well) and calls the API with text/csv", async () => {
    const fetchSpy = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ imported: 1, failed: [] }), { status: 200 }),
    );

    render(withProviders(<Import />, { withRouter: false }));

    const csvBody = "url,alias,ttl\nhttps://example.com,,\n";
    const file = new File([csvBody], "links.csv", { type: "text/csv" });
    await userEvent.upload(screen.getByLabelText(/choose a csv or json file to import/i), file);
    await userEvent.click(screen.getByRole("button", { name: /^import$/i }));

    expect(await screen.findByText(/imported: 1/i)).toBeInTheDocument();
    expect(screen.getByText(/failed: 0/i)).toBeInTheDocument();

    expect(fetchSpy).toHaveBeenCalledTimes(1);
    const [url, opts] = fetchSpy.mock.calls[0] as [string, RequestInit];
    expect(String(url)).toContain("/admin/import");
    const headers = new Headers(opts.headers);
    expect(headers.get("content-type")).toBe("text/csv");
    expect(opts.body).toBe(csvBody);
  });

  it("pastes JSON, calls the API with application/json and shows the summary with a failed row", async () => {
    const fetchSpy = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(
        JSON.stringify({ imported: 1, failed: [{ index: 1, url: "not-a-url", reason: "invalid url" }] }),
        { status: 200 },
      ),
    );

    render(withProviders(<Import />, { withRouter: false }));

    fireEvent.change(screen.getByLabelText(/paste csv or json/i), {
      target: { value: '[{"url":"https://example.com"},{"url":"not-a-url"}]' },
    });
    await userEvent.click(screen.getByRole("button", { name: /^import$/i }));

    expect(await screen.findByText(/imported: 1/i)).toBeInTheDocument();
    expect(screen.getByText(/failed: 1/i)).toBeInTheDocument();
    expect(screen.getByText("not-a-url")).toBeInTheDocument();
    expect(screen.getByText("invalid url")).toBeInTheDocument();

    expect(fetchSpy).toHaveBeenCalledTimes(1);
    const [url, opts] = fetchSpy.mock.calls[0] as [string, RequestInit];
    expect(String(url)).toContain("/admin/import");
    const headers = new Headers(opts.headers);
    expect(headers.get("content-type")).toBe("application/json");
    expect(opts.body).toBe('[{"url":"https://example.com"},{"url":"not-a-url"}]');
  });
});
