import type { ReactNode } from "react";
import {
  Bar,
  BarChart,
  CartesianGrid,
  Cell,
  Pie,
  PieChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useT } from "@/i18n";
import { cn } from "@/lib/utils";
import { toBreakdownData, type BreakdownDatum } from "@/lib/breakdown";
import type { Aggregates } from "@/lib/types";

/**
 * Grayscale ramp already used by the design system (--chart-1..5 tokens in
 * index.css). Reused instead of inventing a new palette to keep the charts
 * consistent with the rest of the panel (badges, icons, etc).
 */
const CHART_COLORS = [
  "var(--color-chart-1)",
  "var(--color-chart-2)",
  "var(--color-chart-3)",
  "var(--color-chart-4)",
  "var(--color-chart-5)",
];

const TOP_N_REFERERS = 8;
const TOP_N_CITIES = 8;

/**
 * Shared recharts tooltip chrome (Quark DS v2): the same popover surface
 * menus/dialogs use, instead of recharts' unstyled default.
 */
const TOOLTIP_STYLE = {
  contentStyle: {
    background: "var(--color-popover)",
    border: "1px solid var(--color-border)",
    borderRadius: 10,
    fontSize: 12,
  },
  labelStyle: { color: "var(--color-popover-foreground)" },
  itemStyle: { color: "var(--color-popover-foreground)" },
} as const;

/** Shared axis tick chrome (Quark DS v2): muted mono 11px, matching table headers/eyebrows. */
const TICK_STYLE = { fontSize: 11, fill: "var(--color-muted-foreground)", fontFamily: "var(--font-mono)" };

function formatDay(day: string): string {
  const [, month, date] = day.split("-");
  return month && date ? `${date}/${month}` : day;
}

interface ChartCardProps {
  title: string;
  empty: boolean;
  emptyLabel: string;
  children: ReactNode;
  /** Lets the per-day chart span the full grid width (it's the panel's hero chart). */
  className?: string;
}

function ChartCard({ title, empty, emptyLabel, children, className }: ChartCardProps) {
  return (
    <Card className={cn("rounded-[12px] [--card-spacing:--spacing(5)]", className)}>
      <CardHeader>
        <CardTitle className="font-sans text-[13.5px] font-semibold">{title}</CardTitle>
      </CardHeader>
      <CardContent>
        {empty ? (
          <p className="flex h-64 items-center justify-center text-center text-sm text-muted-foreground">
            {emptyLabel}
          </p>
        ) : (
          <div className="h-64 w-full">{children}</div>
        )}
      </CardContent>
    </Card>
  );
}

/**
 * Clicks per day (`per_day`), in chronological order — the panel's hero
 * chart (mock isTabAnalytics.html): lime-gradient bars, rounded top corners.
 */
function PerDayChart({ perDay }: { perDay: Record<string, number> }) {
  const t = useT();
  const data = Object.entries(perDay)
    .toSorted(([a], [b]) => a.localeCompare(b))
    .map(([day, count]) => ({ day, count, label: formatDay(day) }));

  return (
    <ChartCard
      title={t("charts.perDayTitle")}
      empty={data.length === 0}
      emptyLabel={t("charts.perDayEmpty")}
      className="md:col-span-2 xl:col-span-3"
    >
      <ResponsiveContainer width="100%" height="100%">
        <BarChart data={data} margin={{ top: 8, right: 12, bottom: 0, left: 0 }}>
          <defs>
            {/* Sanctioned literal rgba (SVG gradient defs only) — plasma-lime fading to transparent, per the mock. */}
            <linearGradient id="perDayFill" x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor="var(--color-chart-1)" />
              <stop offset="100%" stopColor="rgba(198,249,78,.35)" />
            </linearGradient>
          </defs>
          <CartesianGrid strokeDasharray="3 3" stroke="var(--color-border)" vertical={false} />
          <XAxis dataKey="label" tick={TICK_STYLE} stroke="var(--color-muted-foreground)" />
          <YAxis allowDecimals={false} tick={TICK_STYLE} stroke="var(--color-muted-foreground)" width={32} />
          {/* String(label): recharts types the tooltip label as ReactNode, and the
              i18n interpolation only accepts string | number. The other formatters
              in this file already coerce through a template literal. */}
          <Tooltip
            {...TOOLTIP_STYLE}
            formatter={(value) => [`${value}`, t("charts.clicks")]}
            labelFormatter={(label) => t("charts.dayLabel", { label: String(label) })}
          />
          <Bar dataKey="count" name={t("charts.clicks")} fill="url(#perDayFill)" radius={[4, 4, 0, 0]} maxBarSize={48} />
        </BarChart>
      </ResponsiveContainer>
    </ChartCard>
  );
}

interface TopNBarChartProps {
  title: string;
  emptyLabel: string;
  data: BreakdownDatum[];
}

/** Horizontal top-N bar chart, shared shape for referrer/city breakdowns. */
function TopNBarChart({ title, emptyLabel, data }: TopNBarChartProps) {
  const t = useT();
  return (
    <ChartCard title={title} empty={data.length === 0} emptyLabel={emptyLabel}>
      <ResponsiveContainer width="100%" height="100%">
        <BarChart data={data} layout="vertical" margin={{ top: 8, right: 16, bottom: 0, left: 0 }}>
          <CartesianGrid strokeDasharray="3 3" stroke="var(--color-border)" horizontal={false} />
          <XAxis type="number" allowDecimals={false} tick={TICK_STYLE} stroke="var(--color-muted-foreground)" />
          <YAxis type="category" dataKey="label" width={96} tick={TICK_STYLE} stroke="var(--color-muted-foreground)" />
          <Tooltip {...TOOLTIP_STYLE} formatter={(value) => [`${value}`, t("charts.clicks")]} />
          <Bar dataKey="count" name={t("charts.clicks")} fill="var(--color-chart-2)" radius={[0, 4, 4, 0]} />
        </BarChart>
      </ResponsiveContainer>
    </ChartCard>
  );
}

interface DonutChartProps {
  title: string;
  emptyLabel: string;
  data: BreakdownDatum[];
}

/**
 * Clicks per A/B variant (`per_variant`), keyed by the variant's index in
 * `Link.variants` (as a string) — the stats response carries only the index,
 * not the variant URL, so it's labeled positionally ("Variant 0", "Variant 1", …).
 */
function PerVariantChart({ perVariant }: { perVariant: Record<string, number> }) {
  const t = useT();
  const data = Object.entries(perVariant)
    .toSorted(([a], [b]) => Number(a) - Number(b))
    .map(([index, count]) => ({ index, count, label: t("charts.variantLabel", { n: index }) }));

  return (
    <ChartCard
      title={t("charts.perVariantTitle")}
      empty={data.length === 0}
      emptyLabel={t("charts.perVariantEmpty")}
    >
      <ResponsiveContainer width="100%" height="100%">
        <BarChart data={data} margin={{ top: 8, right: 12, bottom: 0, left: 0 }}>
          <CartesianGrid strokeDasharray="3 3" stroke="var(--color-border)" vertical={false} />
          <XAxis dataKey="label" tick={TICK_STYLE} stroke="var(--color-muted-foreground)" />
          <YAxis allowDecimals={false} tick={TICK_STYLE} stroke="var(--color-muted-foreground)" width={32} />
          <Tooltip {...TOOLTIP_STYLE} formatter={(value) => [`${value}`, t("charts.clicks")]} />
          <Bar dataKey="count" name={t("charts.clicks")} fill="var(--color-chart-3)" radius={[4, 4, 0, 0]} />
        </BarChart>
      </ResponsiveContainer>
    </ChartCard>
  );
}

const DONUT_SIZE = 168;
const DONUT_CENTER = DONUT_SIZE / 2;

/**
 * Donut chart, used for the OS breakdown. The pie renders at a fixed pixel
 * size instead of through ResponsiveContainer: recharts 3's polar charts draw
 * zero sectors when they read the container's first (empty) measurement,
 * while cartesian charts recover. The breakdown legend sits beside the ring
 * (own list, not recharts' overlapping outer labels) so short category
 * counts read cleanly and never clip.
 */
function DonutChart({ title, emptyLabel, data }: DonutChartProps) {
  const total = data.reduce((sum, d) => sum + d.count, 0);
  return (
    <ChartCard title={title} empty={data.length === 0} emptyLabel={emptyLabel}>
      <div className="flex h-full items-center gap-5">
        <PieChart width={DONUT_SIZE} height={DONUT_SIZE} className="shrink-0">
          <Pie
            data={data}
            dataKey="count"
            nameKey="label"
            cx={DONUT_CENTER}
            cy={DONUT_CENTER}
            innerRadius={52}
            outerRadius={82}
            paddingAngle={data.length > 1 ? 2 : 0}
            stroke="var(--color-card)"
            strokeWidth={2}
          >
            {data.map((entry, i) => (
              <Cell key={entry.label} fill={CHART_COLORS[i % CHART_COLORS.length]} />
            ))}
          </Pie>
          <Tooltip {...TOOLTIP_STYLE} formatter={(value, name) => [`${value}`, `${name}`]} />
        </PieChart>
        <ul className="flex min-w-0 flex-1 flex-col gap-2 text-sm">
          {data.map((entry, i) => (
            <li key={entry.label} className="flex items-center gap-2">
              <span
                className="size-2.5 shrink-0 rounded-full"
                style={{ backgroundColor: CHART_COLORS[i % CHART_COLORS.length] }}
              />
              <span className="truncate text-muted-foreground">{entry.label}</span>
              <span className="ml-auto font-medium tabular-nums">
                {total > 0 ? Math.round((entry.count / total) * 100) : 0}%
              </span>
            </li>
          ))}
        </ul>
      </div>
    </ChartCard>
  );
}

interface StatsChartsProps {
  aggregates: Aggregates;
}

/**
 * `referer_host()` on the backend returns the untranslated keys `"direct"`
 * and `"other"` for absent/unparseable referrers (see `src/analytics/mod.rs`).
 * Real hostnames pass through unchanged; only those two known keys are mapped
 * to their localized labels.
 */
function relabelReferer(t: ReturnType<typeof useT>, label: string): string {
  if (label === "direct") return t("charts.refererDirect");
  if (label === "other") return t("charts.refererOther");
  return label;
}

/**
 * The recharts half of the stats screen: clicks per day (hero, full width),
 * then OS, referrer, (when present) city, and (when the link has variants
 * with recorded clicks) per-variant breakdowns. Country, device and browser
 * are NOT here — `StatsView` renders those as MeterBar rows (v2: the mock's
 * two side-by-side distribution cards), not recharts. `per_city` is usually
 * empty (most deploys don't send `cf-ipcity`), so its card is omitted
 * entirely rather than shown empty.
 */
export function StatsCharts({ aggregates }: StatsChartsProps) {
  const t = useT();
  const cityData = toBreakdownData(aggregates.per_city, t("charts.unknown"), TOP_N_CITIES);
  const perVariant = aggregates.per_variant ?? {};
  const hasVariantData = Object.keys(perVariant).length > 0;

  return (
    <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
      <PerDayChart perDay={aggregates.per_day} />
      <DonutChart
        title={t("charts.perOsTitle")}
        emptyLabel={t("charts.perOsEmpty")}
        data={toBreakdownData(aggregates.per_os, t("charts.unknown"))}
      />
      <TopNBarChart
        title={t("charts.perRefererTitle")}
        emptyLabel={t("charts.perRefererEmpty")}
        data={toBreakdownData(aggregates.per_referer, t("charts.unknown"), TOP_N_REFERERS, (label) =>
          relabelReferer(t, label),
        )}
      />
      {cityData.length > 0 && (
        <TopNBarChart title={t("charts.perCityTitle")} emptyLabel={t("charts.perCityEmpty")} data={cityData} />
      )}
      {hasVariantData && <PerVariantChart perVariant={perVariant} />}
    </div>
  );
}
