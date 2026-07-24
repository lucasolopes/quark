import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

const TONES = {
  accent: "bg-primary",
  cyan: "bg-chart-2",
  violet: "bg-chart-3",
} as const;

interface MeterBarProps {
  label: ReactNode;
  value?: ReactNode;
  /** 0–100 (clampado). */
  pct: number;
  tone?: keyof typeof TONES;
  className?: string;
}

/** Barra de distribuição do Quark DS (país/dispositivo/navegador). */
export function MeterBar({ label, value, pct, tone = "accent", className }: MeterBarProps) {
  const clamped = Math.max(0, Math.min(100, pct)) / 100;
  return (
    <div className={className}>
      <div className="mb-1.5 flex items-baseline justify-between gap-2 text-[13px]">
        <span className="text-foreground">{label}</span>
        {value != null && <span className="font-mono text-xs text-muted-foreground">{value}</span>}
      </div>
      <div className="h-[7px] overflow-hidden rounded-sm bg-secondary">
        <div
          className={cn("h-full origin-left rounded-sm transition-transform duration-500 ease-out", TONES[tone])}
          style={{ transform: `scaleX(${clamped})` }}
        />
      </div>
    </div>
  );
}
