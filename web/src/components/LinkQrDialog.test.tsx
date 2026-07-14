import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { LinkQrDialog } from "./LinkQrDialog";
import { withProviders } from "@/test-utils";

/** The QR svg draws a background rect (as a `path`) then the foreground modules as a second `path` — take the last one. */
function getFgPath(): SVGPathElement {
  const svg = document.body.querySelector("svg");
  if (!svg) throw new Error("QR svg not found");
  const paths = svg.querySelectorAll('path[shape-rendering="crispEdges"]');
  const path = paths[paths.length - 1];
  if (!path) throw new Error("QR foreground path not found");
  return path as unknown as SVGPathElement;
}

describe("LinkQrDialog", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it("changing the error-correction level re-renders the QR code", async () => {
    render(
      withProviders(<LinkQrDialog code="abc123" url="https://example.com" open onOpenChange={() => {}} />, {
        withRouter: false,
      }),
    );

    const before = getFgPath().getAttribute("d");

    const select = screen.getByLabelText(/error correction/i);
    await userEvent.selectOptions(select, "H");

    const after = getFgPath().getAttribute("d");
    expect(after).not.toEqual(before);
    expect((select as HTMLSelectElement).value).toBe("H");
  });

  it("applies a non-default foreground color to the QR svg path", async () => {
    render(
      withProviders(<LinkQrDialog code="abc123" url="https://example.com" open onOpenChange={() => {}} />, {
        withRouter: false,
      }),
    );

    expect(getFgPath().getAttribute("fill")).toBe("#0A0B0F");

    const fgInput = screen.getByLabelText(/foreground color/i);
    fireEvent.change(fgInput, { target: { value: "#ff0000" } });

    expect(getFgPath().getAttribute("fill")).toBe("#ff0000");
  });

  it("downloads an SVG file named quark-<code>.svg", async () => {
    const createObjectURLMock = vi.fn().mockReturnValue("blob:mock-svg-url");
    const revokeObjectURLMock = vi.fn();
    vi.stubGlobal("URL", { ...URL, createObjectURL: createObjectURLMock, revokeObjectURL: revokeObjectURLMock });

    const clickSpy = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});

    render(
      withProviders(<LinkQrDialog code="abc123" url="https://example.com" open onOpenChange={() => {}} />, {
        withRouter: false,
      }),
    );

    await userEvent.click(screen.getByRole("button", { name: /download svg/i }));

    expect(createObjectURLMock).toHaveBeenCalledOnce();
    expect(clickSpy).toHaveBeenCalledOnce();

    const anchor = clickSpy.mock.instances[0] as HTMLAnchorElement;
    expect(anchor.getAttribute("download")).toBe("quark-abc123.svg");
    expect(anchor.getAttribute("href")).toBe("blob:mock-svg-url");
  });
});

