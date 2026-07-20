import { ChevronDown, ChevronRight } from "lucide-react";
import { useState, type ReactNode } from "react";

interface CollapsibleSectionProps {
  /** Section heading shown next to the chevron toggle. */
  title: string;
  children: ReactNode;
  /** Whether the section starts expanded. Defaults to collapsed. */
  defaultOpen?: boolean;
}

/**
 * Collapsible bordered section shell shared by the create and edit link
 * dialogs (scheduling, app redirect, password, UTM, ...). Manages its own
 * open state locally; the dialogs mount it fresh on each open, so it always
 * starts from `defaultOpen`.
 */
export function CollapsibleSection({ title, children, defaultOpen = false }: CollapsibleSectionProps) {
  const [open, setOpen] = useState(defaultOpen);
  return (
    <div className="flex flex-col gap-2 rounded-lg border border-input p-2.5">
      <button
        type="button"
        className="flex items-center gap-1.5 text-sm font-medium"
        aria-expanded={open}
        onClick={() => setOpen((open) => !open)}
      >
        {open ? (
          <ChevronDown className="size-4 text-muted-foreground" aria-hidden />
        ) : (
          <ChevronRight className="size-4 text-muted-foreground" aria-hidden />
        )}
        {title}
      </button>

      {open && <div className="flex flex-col gap-3 pt-1">{children}</div>}
    </div>
  );
}
