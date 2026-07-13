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
import type { Aggregates } from "@/lib/types";

// Rampa de cinza já usada pelo design system (tokens --chart-1..5 no
// index.css). Reaproveitar em vez de inventar uma paleta nova mantém os
// gráficos consistentes com o resto do painel (badges, ícones etc).
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

/** Cliques por dia (`per_day`), em ordem cronológica. */
function PerDayChart({ perDay }: { perDay: Record<string, number> }) {
  const data = Object.entries(perDay)
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([day, count]) => ({ day, count, label: formatDay(day) }));

  return (
    <ChartCard title="Cliques por dia" empty={data.length === 0} emptyLabel="Sem dados de dia ainda.">
      <ResponsiveContainer width="100%" height="100%">
        <LineChart data={data} margin={{ top: 8, right: 12, bottom: 0, left: 0 }}>
          <CartesianGrid strokeDasharray="3 3" stroke="var(--color-border)" vertical={false} />
          <XAxis dataKey="label" tick={{ fontSize: 12 }} stroke="var(--color-muted-foreground)" />
          <YAxis allowDecimals={false} tick={{ fontSize: 12 }} stroke="var(--color-muted-foreground)" width={32} />
          <Tooltip
            formatter={(value) => [`${value}`, "Cliques"]}
            labelFormatter={(label) => `Dia ${label}`}
          />
          <Line
            type="monotone"
            dataKey="count"
            name="Cliques"
            stroke="var(--color-chart-2)"
            strokeWidth={2}
            dot={{ r: 3 }}
          />
        </LineChart>
      </ResponsiveContainer>
    </ChartCard>
  );
}

/** Top-N países por volume de cliques (`per_country`), ordem decrescente. */
function PerCountryChart({ perCountry }: { perCountry: Record<string, number> }) {
  const data = Object.entries(perCountry)
    .sort(([, a], [, b]) => b - a)
    .slice(0, TOP_N_COUNTRIES)
    .map(([country, count]) => ({ country: country || "Desconhecido", count }));

  return (
    <ChartCard title="Cliques por país" empty={data.length === 0} emptyLabel="Sem dados de país ainda.">
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
          <Tooltip formatter={(value) => [`${value}`, "Cliques"]} />
          <Bar dataKey="count" name="Cliques" fill="var(--color-chart-2)" radius={[0, 4, 4, 0]} />
        </BarChart>
      </ResponsiveContainer>
    </ChartCard>
  );
}

/** Distribuição por dispositivo (`per_device`), como rosca. */
function PerDeviceChart({ perDevice }: { perDevice: Record<string, number> }) {
  const data = Object.entries(perDevice)
    .sort(([, a], [, b]) => b - a)
    .map(([device, count]) => ({ device: device || "Desconhecido", count }));

  return (
    <ChartCard title="Cliques por dispositivo" empty={data.length === 0} emptyLabel="Sem dados de dispositivo ainda.">
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

/** Os três gráficos da tela de stats: cliques por dia, por país e por dispositivo. */
export function StatsCharts({ aggregates }: StatsChartsProps) {
  return (
    <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
      <PerDayChart perDay={aggregates.per_day} />
      <PerCountryChart perCountry={aggregates.per_country} />
      <PerDeviceChart perDevice={aggregates.per_device} />
    </div>
  );
}
