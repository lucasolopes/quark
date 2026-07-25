import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { Dialog, DialogContent, DialogFooter, DialogTitle } from "@/components/ui/dialog";

/**
 * Baseline `DialogContent` output with no `fullScreenOnMobile` — pinned
 * verbatim so a future edit that accidentally changes the default (small
 * dialogs: confirms, QR, tokens, invites, ...) output fails loudly instead
 * of shipping a silent regression.
 */
const DEFAULT_CONTENT_CLASSES =
  "fixed top-1/2 left-1/2 z-50 grid w-full max-w-[540px] -translate-x-1/2 -translate-y-1/2 gap-4 rounded-[16px] border border-input bg-card p-6 text-sm text-popover-foreground shadow-modal data-open:animate-rise data-closed:animate-rise-out outline-none";

/** Same guard for `DialogFooter`'s default (non-full-screen) output. */
const DEFAULT_FOOTER_CLASSES =
  "-mx-6 -mb-6 flex flex-col-reverse gap-2 rounded-b-[16px] border-t bg-muted/50 p-6 sm:flex-row sm:justify-end";

describe("DialogContent — fullScreenOnMobile", () => {
  it("without the prop, keeps today's exact centered-card classes", () => {
    render(
      <Dialog open onOpenChange={() => {}}>
        <DialogContent data-testid="content">
          <DialogTitle>Title</DialogTitle>
        </DialogContent>
      </Dialog>,
    );
    expect(screen.getByTestId("content").className).toBe(DEFAULT_CONTENT_CLASSES);
  });

  it("with the prop, adds the below-sm full-screen sheet classes", () => {
    render(
      <Dialog open onOpenChange={() => {}}>
        <DialogContent fullScreenOnMobile data-testid="content">
          <DialogTitle>Title</DialogTitle>
        </DialogContent>
      </Dialog>,
    );
    const className = screen.getByTestId("content").className;
    for (const token of [
      "max-sm:inset-0",
      "max-sm:h-dvh",
      "max-sm:max-w-none",
      "max-sm:rounded-none",
      "max-sm:translate-x-0",
      "max-sm:translate-y-0",
      "max-sm:grid-rows-[minmax(0,1fr)]",
    ]) {
      expect(className).toContain(token);
    }
  });

  it("with the prop, still carries the base centered/animation classes (they don't fight — see comment in dialog.tsx)", () => {
    render(
      <Dialog open onOpenChange={() => {}}>
        <DialogContent fullScreenOnMobile data-testid="content">
          <DialogTitle>Title</DialogTitle>
        </DialogContent>
      </Dialog>,
    );
    const className = screen.getByTestId("content").className;
    expect(className).toContain("data-open:animate-rise");
    expect(className).toContain("data-closed:animate-rise-out");
  });
});

describe("DialogFooter — sticky full-screen mode", () => {
  it("without an ancestor fullScreenOnMobile, keeps today's exact footer classes", () => {
    render(
      <Dialog open onOpenChange={() => {}}>
        <DialogContent>
          <DialogTitle>Title</DialogTitle>
          <DialogFooter data-testid="footer" />
        </DialogContent>
      </Dialog>,
    );
    expect(screen.getByTestId("footer").className).toBe(DEFAULT_FOOTER_CLASSES);
  });

  it("inside a fullScreenOnMobile DialogContent, becomes an opaque square-cornered bar below sm", () => {
    render(
      <Dialog open onOpenChange={() => {}}>
        <DialogContent fullScreenOnMobile>
          <DialogTitle>Title</DialogTitle>
          <DialogFooter data-testid="footer" />
        </DialogContent>
      </Dialog>,
    );
    const className = screen.getByTestId("footer").className;
    for (const token of ["max-sm:bg-card", "max-sm:rounded-b-none"]) {
      expect(className).toContain(token);
    }
  });
});
