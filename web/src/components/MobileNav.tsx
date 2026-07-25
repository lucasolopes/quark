import type { ReactNode } from "react";
import { Dialog as DialogPrimitive } from "@base-ui/react/dialog";
import { XIcon } from "lucide-react";
import { NavLink } from "react-router-dom";
import type { NavGroup } from "@/app/Shell";
import { Button } from "@/components/ui/button";
import { DialogOverlay, DialogPortal } from "@/components/ui/dialog";
import { useT } from "@/i18n";
import { cn } from "@/lib/utils";

interface MobileNavProps {
  /** Controlled open state — `Shell` owns `mobileNavOpen`, this never manages its own. */
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The RBAC-filtered nav groups `Shell` already built for the sidebar — the single
   * source of truth; this component only renders them, it never recomputes its own copy. */
  groups: NavGroup[];
  /** Rendered above the nav groups (logo/wordmark, WorkspaceSwitcher, in `Shell`). */
  children?: ReactNode;
  /** Rendered below the nav groups (connected line, user card, language/theme controls, in `Shell`). */
  footer?: ReactNode;
}

/**
 * Left-side navigation drawer for < md viewports. Built directly on the Base
 * UI Dialog primitive (`Root`/`Portal`/`Popup`/`Close`) rather than the
 * centered `DialogContent` in `ui/dialog.tsx` — a left-pinned, full-height
 * panel needs its own positioning, so only the shared, layout-agnostic
 * `DialogOverlay` scrim is reused from there.
 *
 * Dismisses the same way any Base UI dialog does (Esc, outside/scrim press,
 * its own close button) plus one extra path: clicking a nav item both
 * navigates (native `NavLink` behavior — this never calls `preventDefault`)
 * and closes the drawer.
 */
export function MobileNav({ open, onOpenChange, groups, children, footer }: MobileNavProps) {
  const t = useT();

  return (
    <DialogPrimitive.Root open={open} onOpenChange={onOpenChange}>
      <DialogPortal>
        <DialogOverlay />
        <DialogPrimitive.Popup
          className={cn(
            "fixed inset-y-0 left-0 z-50 flex h-dvh w-[280px] flex-col overflow-y-auto border-r border-sidebar-border bg-sidebar px-3 py-4 outline-none",
            "data-open:animate-slide-in-left data-closed:animate-slide-out-left",
          )}
        >
          <DialogPrimitive.Title className="sr-only">{t("shell.mobileNavTitle")}</DialogPrimitive.Title>

          <DialogPrimitive.Close
            render={
              <Button
                variant="ghost"
                size="icon-sm"
                className="size-11 mb-3"
                aria-label={t("shell.closeMenu")}
              />
            }
          >
            <XIcon className="size-4" aria-hidden="true" />
          </DialogPrimitive.Close>

          {children}

          <nav className="flex flex-col gap-4">
            {groups.map((group) => (
              <div key={group.label} className="flex flex-col gap-1">
                <div className="px-3 pb-2 font-mono text-[10px] font-medium tracking-[0.12em] text-sidebar-foreground/45 uppercase">
                  {group.label}
                </div>
                {group.items.map(({ to, label, icon: Icon }) => (
                  <NavLink
                    key={to}
                    to={to}
                    onClick={() => onOpenChange(false)}
                    className={({ isActive }) =>
                      cn(
                        "flex min-h-11 items-center gap-3 rounded-[9px] px-[11px] py-[9px] text-[14.5px] font-medium transition-colors",
                        isActive
                          ? "bg-sidebar-accent text-sidebar-accent-foreground"
                          : "text-sidebar-foreground/70 hover:bg-surface-hover",
                      )
                    }
                  >
                    <Icon className="size-[18px] shrink-0" aria-hidden="true" />
                    <span>{label}</span>
                  </NavLink>
                ))}
              </div>
            ))}
          </nav>

          <div className="flex-1" />

          {footer}
        </DialogPrimitive.Popup>
      </DialogPortal>
    </DialogPrimitive.Root>
  );
}
