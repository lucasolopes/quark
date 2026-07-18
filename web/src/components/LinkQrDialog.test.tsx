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

  it("downloads an SVG file named quark-<code>.svg and revokes the blob URL on a deferred tick", () => {
    const createObjectURLMock = vi.fn().mockReturnValue("blob:mock-svg-url");
    const revokeObjectURLMock = vi.fn();
    vi.stubGlobal("URL", { ...URL, createObjectURL: createObjectURLMock, revokeObjectURL: revokeObjectURLMock });

    const clickSpy = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});

    render(
      withProviders(<LinkQrDialog code="abc123" url="https://example.com" open onOpenChange={() => {}} />, {
        withRouter: false,
      }),
    );

    // Plain fireEvent.click (not userEvent) so the deferred revoke can be
    // driven with fake timers without also needing userEvent's own timer
    // integration.
    vi.useFakeTimers();
    fireEvent.click(screen.getByRole("button", { name: /download svg/i }));

    expect(createObjectURLMock).toHaveBeenCalledOnce();
    expect(clickSpy).toHaveBeenCalledOnce();

    const anchor = clickSpy.mock.instances[0] as HTMLAnchorElement;
    expect(anchor.getAttribute("download")).toBe("quark-abc123.svg");
    expect(anchor.getAttribute("href")).toBe("blob:mock-svg-url");

    // The blob URL must not be revoked synchronously right after click(): it
    // is only revoked once the deferred (setTimeout) tick runs.
    expect(revokeObjectURLMock).not.toHaveBeenCalled();

    vi.runAllTimers();

    expect(revokeObjectURLMock).toHaveBeenCalledOnce();
    expect(revokeObjectURLMock).toHaveBeenCalledWith("blob:mock-svg-url");

    vi.useRealTimers();
  });

  it("applies the chosen background color when exporting the PNG", async () => {
    const createObjectURLMock = vi.fn().mockReturnValue("blob:mock-svg-url");
    const revokeObjectURLMock = vi.fn();
    vi.stubGlobal("URL", { ...URL, createObjectURL: createObjectURLMock, revokeObjectURL: revokeObjectURLMock });

    const fakeCtx = {
      fillStyle: "",
      fillRect: vi.fn(),
      drawImage: vi.fn(),
    };
    const getContextSpy = vi
      .spyOn(HTMLCanvasElement.prototype, "getContext")
      .mockReturnValue(fakeCtx as unknown as CanvasRenderingContext2D);
    const toDataURLSpy = vi
      .spyOn(HTMLCanvasElement.prototype, "toDataURL")
      .mockReturnValue("data:image/png;base64,mock");

    const clickSpy = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});

    // Auto-fire the onload callback as soon as `src` is assigned, mimicking
    // the browser's image-decode completion (jsdom does not decode images).
    const originalImage = window.Image;
    class FakeImage {
      onload: (() => void) | null = null;
      set src(_value: string) {
        this.onload?.();
      }
    }
    vi.stubGlobal("Image", FakeImage as unknown as typeof Image);

    render(
      withProviders(<LinkQrDialog code="abc123" url="https://example.com" open onOpenChange={() => {}} />, {
        withRouter: false,
      }),
    );

    const bgInput = screen.getByLabelText(/background color/i);
    fireEvent.change(bgInput, { target: { value: "#ff00aa" } });

    await userEvent.click(screen.getByRole("button", { name: /download png/i }));

    expect(getContextSpy).toHaveBeenCalled();
    // fillStyle must be set to the chosen bgColor before fillRect paints the
    // background, and before the QR image is drawn on top.
    expect(fakeCtx.fillStyle).toBe("#ff00aa");
    expect(fakeCtx.fillRect).toHaveBeenCalledOnce();
    expect(fakeCtx.drawImage).toHaveBeenCalledOnce();
    expect(clickSpy).toHaveBeenCalledOnce();
    expect(revokeObjectURLMock).toHaveBeenCalledWith("blob:mock-svg-url");

    toDataURLSpy.mockRestore();
    getContextSpy.mockRestore();
    vi.stubGlobal("Image", originalImage);
  });
});

