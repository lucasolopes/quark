import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

interface StatCardProps {
  value: ReactNode;
  label: ReactNode;
  /** Numeral em lime (métrica-herói); sem accent fica no strong. */
  accent?: boolean;
  className?: string;
}

/** KPI do Quark DS: numeral display grande + label muted, em card hairline. */
export function StatCard({ value, label, accent = false, className }: StatCardProps) {
  return (
    <div className={cn("rounded-lg border border-border bg-card p-[18px] shadow-card", className)}>
      <div className="text-[12.5px] text-muted-foreground">{label}</div>
      <div className={cn("mt-1.5 font-heading text-stat font-bold", accent ? "text-brand-ink" : "text-strong")}>{value}</div>
    </div>
  );
}
