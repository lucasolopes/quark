// StatCard preview — hero KPI numerals (Space Grotesk display) over hairline cards.
import { StatCard } from "web";

export function KpiRow() {
  return (
    <div className="grid w-[640px] grid-cols-3 gap-4">
      <StatCard value="48,215" label="Clicks · last 30 days" accent />
      <StatCard value="1,982" label="Active links" />
      <StatCard value="96.4%" label="Redirect uptime" />
    </div>
  );
}

export function AccentVsDefault() {
  return (
    <div className="flex w-[420px] gap-4">
      <StatCard className="flex-1" value="12,408" label="Clicks today" accent />
      <StatCard className="flex-1" value="315 ms" label="p99 redirect" />
    </div>
  );
}
