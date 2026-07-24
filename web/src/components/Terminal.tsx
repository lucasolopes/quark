import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

interface TerminalProps {
  title?: string;
  children: ReactNode;
  className?: string;
}

/** Janela de terminal do Quark DS (traffic lights + corpo mono). */
export function Terminal({ title = "quark — zsh", children, className }: TerminalProps) {
  return (
    <div className={cn("overflow-hidden rounded-lg border border-border bg-surface-input shadow-modal", className)}>
      <div className="flex items-center gap-2 border-b border-border bg-white/[0.02] px-4 py-3">
        {(["#ff5f57", "#febc2e", "#28c840"] as const).map((c) => (
          <span
            key={c}
            data-testid="traffic-light"
            aria-hidden="true"
            className="size-[11px] rounded-full"
            style={{ background: c }}
          />
        ))}
        <span className="ml-2 font-mono text-xs text-muted-foreground">{title}</span>
      </div>
      <pre className="m-0 overflow-x-auto p-5 font-mono text-[13.5px] leading-[1.85] whitespace-pre-wrap text-foreground/85">{children}</pre>
    </div>
  );
}
