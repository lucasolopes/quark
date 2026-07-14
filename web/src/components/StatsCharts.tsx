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

/** Top-N countries by click volume (`per_country`), descending order. */
function PerCountryChart({ perCountry }: { perCountry: Record<string, number> }) {
  const t = useT();
  const data = Object.entries(perCountry)
    .sort(([, a], [, b]) => b - a)
    .slice(0, TOP_N_COUNTRIES)
    .map(([country, count]) => ({ country: country || t("charts.unknown"), count }));

  return (
    <ChartCard
      title={t("charts.perCountryTitle")}
      empty={data.length === 0}
      emptyLabel={t("charts.perCountryEmpty")}
    >
      <ResponsiveContainer width="100%" height="100%">
        <BarChart data={data} layout="vertical" margin={{ top: 8, right: 16, bottom: 0, left: 0 }}>
          <CartesianGrid strokeDasharray="3 3" stroke="var(--color-border)" horizontal={false} />
          <XAxis type="number" allowDecimals={false} tick={{ fontSize: 12 }} stroke="var(--color-muted-foreground)" />
          <YAxis
            type="category"
            dataKey="country"
            width={64}
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

/** Device distribution (`per_device`), as a donut chart. */
function PerDeviceChart({ perDevice }: { perDevice: Record<string, number> }) {
  const t = useT();
  const data = Object.entries(perDevice)
    .sort(([, a], [, b]) => b - a)
    .map(([device, count]) => ({ device: device || t("charts.unknown"), count }));

  return (
    <ChartCard
      title={t("charts.perDeviceTitle")}
      empty={data.length === 0}
      emptyLabel={t("charts.perDeviceEmpty")}
    >
      <ResponsiveContainer width="100%" height="100%">
        <PieChart>
          <Pie
            data={data}
            dataKey="count"
            nameKey="device"
            innerRadius="55%"
            outerRadius="85%"
            paddingAngle={2}
            label={({ name, percent }) => `${name} ${Math.round((percent ?? 0) * 100)}%`}
            labelLine={false}
          >
            {data.map((entry, i) => (
              <Cell key={entry.device} fill={CHART_COLORS[i % CHART_COLORS.length]} />
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

/** The three charts on the stats screen: clicks per day, per country and per device. */
export function StatsCharts({ aggregates }: StatsChartsProps) {
  return (
    <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
      <PerDayChart perDay={aggregates.per_day} />
      <PerCountryChart perCountry={aggregates.per_country} />
      <PerDeviceChart perDevice={aggregates.per_device} />
    </div>
  );
}
