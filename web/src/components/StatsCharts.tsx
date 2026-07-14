import type { ReactNode } from "react";
import {
  Bar,
  BarChart,
  CartesianGrid,
  Cell,
  Line,
  LineChart,
  Pie,
  PieChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useT } from "@/i18n";
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

const TOP_N_COUNTRIES = 8;
const TOP_N_REFERERS = 8;
const TOP_N_CITIES = 8;

function formatDay(day: string): string {
  const [, month, date] = day.split("-");
  return month && date ? `${date}/${month}` : day;
}

interface ChartCardProps {
  title: string;
  empty: boolean;
  emptyLabel: string;
  children: ReactNode;
}

function ChartCard({ title, empty, emptyLabel, children }: ChartCardProps) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>{title}</CardTitle>
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

/** Clicks per day (`per_day`), in chronological order. */
function PerDayChart({ perDay }: { perDay: Record<string, number> }) {
  const t = useT();
  const data = Object.entries(perDay)
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([day, count]) => ({ day, count, label: formatDay(day) }));

  return (
    <ChartCard title={t("charts.perDayTitle")} empty={data.length === 0} emptyLabel={t("charts.perDayEmpty")}>
      <ResponsiveContainer width="100%" height="100%">
        <LineChart data={data} margin={{ top: 8, right: 12, bottom: 0, left: 0 }}>
          <CartesianGrid strokeDasharray="3 3" stroke="var(--color-border)" vertical={false} />
          <XAxis dataKey="label" tick={{ fontSize: 12 }} stroke="var(--color-muted-foreground)" />
          <YAxis allowDecimals={false} tick={{ fontSize: 12 }} stroke="var(--color-muted-foreground)" width={32} />
          <Tooltip
            formatter={(value) => [`${value}`, t("charts.clicks")]}
            labelFormatter={(label) => t("charts.dayLabel", { label })}
          />
          <Line
            type="monotone"
            dataKey="count"
            name={t("charts.clicks")}
            stroke="var(--color-chart-2)"
            strokeWidth={2}
            dot={{ r: 3 }}
          />
        </LineChart>
      </ResponsiveContainer>
    </ChartCard>
  );
}

interface BreakdownDatum {
  label: string;
  count: number;
}

/** `per_*` map turned into sorted, top-N chart data with the unknown-key fallback applied. */
function toBreakdownData(
  map: Record<string, number>,
  unknownLabel: string,
  topN?: number,
  relabel?: (label: string) => string,
): BreakdownDatum[] {
  const sorted = Object.entries(map)
    .sort(([, a], [, b]) => b - a)
    .map(([label, count]) => ({ label: label ? (relabel ? relabel(label) : label) : unknownLabel, count }));
  return topN === undefined ? sorted : sorted.slice(0, topN);
}

interface TopNBarChartProps {
  title: string;
  emptyLabel: string;
  data: BreakdownDatum[];
}

/** Horizontal top-N bar chart, shared shape for country/referrer/city breakdowns. */
function TopNBarChart({ title, emptyLabel, data }: TopNBarChartProps) {
  const t = useT();
  return (
    <ChartCard title={title} empty={data.length === 0} emptyLabel={emptyLabel}>
      <ResponsiveContainer width="100%" height="100%">
        <BarChart data={data} layout="vertical" margin={{ top: 8, right: 16, bottom: 0, left: 0 }}>
          <CartesianGrid strokeDasharray="3 3" stroke="var(--color-border)" horizontal={false} />
          <XAxis type="number" allowDecimals={false} tick={{ fontSize: 12 }} stroke="var(--color-muted-foreground)" />
          <YAxis
            type="category"
            dataKey="label"
            width={96}
            tick={{ fontSize: 12 }}
            stroke="var(--color-muted-foreground)"
          />
          <Tooltip formatter={(value) => [`${value}`, t("charts.clicks")]} />
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

/** Donut chart, shared shape for device/OS/browser breakdowns. */
function DonutChart({ title, emptyLabel, data }: DonutChartProps) {
  return (
    <ChartCard title={title} empty={data.length === 0} emptyLabel={emptyLabel}>
      <ResponsiveContainer width="100%" height="100%">
        <PieChart>
          <Pie
            data={data}
            dataKey="count"
            nameKey="label"
            innerRadius="55%"
            outerRadius="85%"
            paddingAngle={2}
            label={({ name, percent }) => `${name} ${Math.round((percent ?? 0) * 100)}%`}
            labelLine={false}
          >
            {data.map((entry, i) => (
              <Cell key={entry.label} fill={CHART_COLORS[i % CHART_COLORS.length]} />
            ))}
          </Pie>
          <Tooltip formatter={(value, name) => [`${value}`, `${name}`]} />
        </PieChart>
      </ResponsiveContainer>
    </ChartCard>
  );
}

interface StatsChartsProps {
  aggregates: Aggregates;
}

/**
 * Every chart on the stats screen: clicks per day, then top-N and distribution
 * breakdowns for country, device, OS, browser, referrer and (when present) city.
 * `per_city` is usually empty (most deploys don't send `cf-ipcity`), so its
 * card is omitted entirely rather than shown empty.
 */
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

export function StatsCharts({ aggregates }: StatsChartsProps) {
  const t = useT();
  const cityData = toBreakdownData(aggregates.per_city, t("charts.unknown"), TOP_N_CITIES);

  return (
    <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
      <PerDayChart perDay={aggregates.per_day} />
      <TopNBarChart
        title={t("charts.perCountryTitle")}
        emptyLabel={t("charts.perCountryEmpty")}
        data={toBreakdownData(aggregates.per_country, t("charts.unknown"), TOP_N_COUNTRIES)}
      />
      <DonutChart
        title={t("charts.perDeviceTitle")}
        emptyLabel={t("charts.perDeviceEmpty")}
        data={toBreakdownData(aggregates.per_device, t("charts.unknown"))}
      />
      <DonutChart
        title={t("charts.perOsTitle")}
        emptyLabel={t("charts.perOsEmpty")}
        data={toBreakdownData(aggregates.per_os, t("charts.unknown"))}
      />
      <DonutChart
        title={t("charts.perBrowserTitle")}
        emptyLabel={t("charts.perBrowserEmpty")}
        data={toBreakdownData(aggregates.per_browser, t("charts.unknown"))}
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
    </div>
  );
}
