import { ChevronDown, ChevronRight } from "lucide-react";
import { useState, type ReactNode } from "react";

interface CollapsibleSectionProps {
  /** Section heading shown next to the chevron toggle. A plain string for most
   * sections; a fragment for ones that append a live count (e.g. rules). */
  title: ReactNode;
  children: ReactNode;
  /** Whether the section starts expanded. Defaults to collapsed. */
  defaultOpen?: boolean;
}

/**
 * Collapsible hairline section shell shared by the create and edit link
 * dialogs (scheduling, app redirect, password, UTM, rules, variants, ...).
 * Manages its own open state locally; the dialogs mount it fresh on each
 * open, so it always starts from `defaultOpen`.
 */
export function CollapsibleSection({ title, children, defaultOpen = false }: CollapsibleSectionProps) {
  const [open, setOpen] = useState(defaultOpen);
  return (
    <div className="flex flex-col gap-2 rounded-lg border border-border p-2.5">
      <button
        type="button"
        className="flex items-center gap-1.5 font-mono text-xs font-medium tracking-wide text-muted-foreground uppercase"
        aria-expanded={open}
        onClick={() => setOpen((prev) => !prev)}
      >
        {open ? (
          <ChevronDown className="size-3.5" aria-hidden />
        ) : (
          <ChevronRight className="size-3.5" aria-hidden />
        )}
        {title}
      </button>

      {open && <div className="flex flex-col gap-3 pt-1">{children}</div>}
    </div>
  );
}
