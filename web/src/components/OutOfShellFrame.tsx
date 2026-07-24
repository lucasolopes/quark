import type { ReactElement, ReactNode } from "react";
import { QuarkMark } from "@/components/brand/QuarkMark";

interface OutOfShellFrameProps {
  title: ReactNode;
  subtitle?: ReactNode;
  /** Chrome anchored top-right over the backdrop (only `Login`'s `LanguageSwitcher` uses it today). */
  topRight?: ReactNode;
  /** The bordered card's content. */
  children: ReactNode;
}

/**
 * Backdrop shared by every page that renders outside the authenticated Shell:
 * glow + dot-grid decorative layers (`aria-hidden`, `-z-10` so they never
 * paint over content regardless of DOM order) plus the full-height centered
 * flex column. Exported on its own — not just used by `OutOfShellFrame` below
 * — because `AcceptInvite`'s loading state centers a bare spinner on this
 * same backdrop, without the title/card `OutOfShellFrame` wraps around it.
 */
export function OutOfShellBackdrop({
  topRight,
  children,
}: {
  topRight?: ReactNode;
  children: ReactNode;
}): ReactElement {
  return (
    <div className="relative min-h-svh overflow-hidden bg-background">
      <div aria-hidden="true" className="pointer-events-none absolute inset-0 -z-10 bg-hero-glow" />
      <div aria-hidden="true" className="pointer-events-none absolute inset-0 -z-10 bg-dot-grid" />
      {topRight && <div className="absolute right-4 top-4">{topRight}</div>}
      <div className="flex min-h-svh items-center justify-center p-4">{children}</div>
    </div>
  );
}

/**
 * Shared frame for the pages that render outside the authenticated Shell —
 * `Login`, `Onboarding`, `AcceptInvite` (its two content-bearing states; the
 * loading state uses the bare `OutOfShellBackdrop` instead, since it has no
 * title or card). Renders the backdrop, a centered `max-w-[400px]` column
 * with the glowing glyph, a display title and an optional muted subtitle,
 * then the bordered card holding `children`. Migrating the three screens to
 * this frame must not change what any of them render — see each screen's own
 * test suite for the regression net.
 */
export function OutOfShellFrame({ title, subtitle, topRight, children }: OutOfShellFrameProps): ReactElement {
  return (
    <OutOfShellBackdrop topRight={topRight}>
      <div className="w-full max-w-[400px] animate-rise">
        <div className="mb-[30px] flex flex-col items-center text-center">
          <QuarkMark className="mb-[18px] size-[42px] text-primary glow-glyph" />
          <h1 className="font-heading text-[26px] font-bold tracking-display text-strong">{title}</h1>
          {subtitle && <p className="mt-2 text-[14.5px] text-muted-foreground">{subtitle}</p>}
        </div>
        <div className="w-full rounded-[16px] border border-input bg-card p-6 shadow-modal">{children}</div>
      </div>
    </OutOfShellBackdrop>
  );
}
