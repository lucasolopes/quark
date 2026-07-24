import { AlertTriangle, MousePointerClick, RotateCw } from "lucide-react";
import { lazy, Suspense } from "react";
import { Button } from "@/components/ui/button";
import { Card, CardAction, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { MeterBar } from "@/components/MeterBar";
import { RecentEventsTable } from "@/components/RecentEventsTable";
import { StatCard } from "@/components/StatCard";
import { useT } from "@/i18n";

// recharts is heavy; keep it out of the main bundle by loading the charts
// component only when a link's stats are actually rendered.
const StatsCharts = lazy(() => import("@/components/StatsCharts").then((m) => ({ default: m.StatsCharts })));
import { formatDateTime, formatNumber } from "@/lib/format";
import { toBreakdownData, TOP_N_COUNTRIES } from "@/lib/breakdown";
import { useStats } from "@/lib/queries";

interface MeterRow {
  label: string;
  pct: number;
}

/** `per_*` map turned into MeterBar rows: sorted, top-N, each entry's share of the total. */
function toMeterRows(map: Record<string, number>, unknownLabel: string, topN?: number): MeterRow[] {
  const data = toBreakdownData(map, unknownLabel, topN);
  const total = data.reduce((sum, d) => sum + d.count, 0);
  return data.map((d) => ({ label: d.label, pct: total > 0 ? (d.count / total) * 100 : 0 }));
}

interface MeterListProps {
  rows: MeterRow[];
  tone: "cyan" | "violet" | "accent";
  emptyLabel: string;
}

/** A distribution card's body: MeterBar rows (% mono at the right), or a muted empty message. */
function MeterList({ rows, tone, emptyLabel }: MeterListProps) {
  if (rows.length === 0) {
    return <p className="py-2 text-sm text-muted-foreground">{emptyLabel}</p>;
  }
  return (
    <div className="flex flex-col gap-3">
      {rows.map((row) => (
        <MeterBar key={row.label} label={row.label} value={`${Math.round(row.pct)}%`} pct={row.pct} tone={tone} />
      ))}
    </div>
  );
}

export function StatsView({ code }: { code: string }) {
  const t = useT();
  const query = useStats(code);

  return (
    <div className="flex flex-col gap-4">
      {query.isPending && <StatsSkeleton />}

      {query.isError && (
        <Card className="border-destructive/30">
          <CardContent className="flex flex-col items-center gap-3 py-8 text-center">
            <AlertTriangle className="size-8 text-destructive" aria-hidden="true" />
            <div>
              <p className="font-medium">{t("stats.loadError")}</p>
              <p className="text-sm text-muted-foreground">
                {query.error instanceof Error ? query.error.message : t("common.retryHint")}
              </p>
            </div>
            <div className="flex gap-2">
              <Button variant="outline" onClick={() => query.refetch()}>
                <RotateCw className="size-4" />
                {t("common.retry")}
              </Button>
            </div>
          </CardContent>
        </Card>
      )}

      {query.isSuccess && query.data.aggregates.total === 0 && (
        <Card>
          <CardContent className="flex flex-col items-center gap-3 py-12 text-center">
            <MousePointerClick className="size-8 text-muted-foreground" aria-hidden="true" />
            <div>
              <p className="font-medium">{t("stats.emptyTitle")}</p>
              <p className="text-sm text-muted-foreground">{t("stats.emptySubtitle")}</p>
            </div>
          </CardContent>
        </Card>
      )}

      {query.isSuccess && query.data.aggregates.total > 0 && (
        <>
          <div className="grid grid-cols-2 lg:grid-cols-4 gap-3.5">
            <StatCard label={t("stats.totalClicks")} value={formatNumber(query.data.aggregates.total)} accent />
            <StatCard
              label={t("stats.botsExcluded")}
              value={<span className="text-[22px]">{formatNumber(query.data.aggregates.bots)}</span>}
            />
            <StatCard
              label={t("stats.firstClick")}
              value={<span className="text-[22px]">{formatDateTime(query.data.aggregates.first_ts)}</span>}
            />
            <StatCard
              label={t("stats.lastClick")}
              value={<span className="text-[22px]">{formatDateTime(query.data.aggregates.last_ts)}</span>}
            />
          </div>

          <p className="text-sm text-muted-foreground">{t("stats.chartsHumanOnlyHint")}</p>

          <Suspense fallback={<ChartsSkeleton />}>
            <StatsCharts aggregates={query.data.aggregates} />
          </Suspense>

          <div className="grid gap-4 md:grid-cols-2">
            <Card>
              <CardHeader>
                <CardTitle>{t("charts.perCountryTitle")}</CardTitle>
              </CardHeader>
              <CardContent>
                <MeterList
                  rows={toMeterRows(query.data.aggregates.per_country, t("charts.unknown"), TOP_N_COUNTRIES)}
                  tone="cyan"
                  emptyLabel={t("charts.perCountryEmpty")}
                />
              </CardContent>
            </Card>

            <Card>
              <CardHeader>
                <CardTitle>{t("charts.perDeviceTitle")}</CardTitle>
              </CardHeader>
              <CardContent className="flex flex-col gap-4">
                <MeterList
                  rows={toMeterRows(query.data.aggregates.per_device, t("charts.unknown"))}
                  tone="violet"
                  emptyLabel={t("charts.perDeviceEmpty")}
                />
                <div>
                  <CardTitle className="mb-3">{t("charts.perBrowserTitle")}</CardTitle>
                  <MeterList
                    rows={toMeterRows(query.data.aggregates.per_browser, t("charts.unknown"))}
                    tone="accent"
                    emptyLabel={t("charts.perBrowserEmpty")}
                  />
                </div>
              </CardContent>
            </Card>
          </div>

          <Card className="py-0">
            <CardHeader className="pt-4">
              <CardTitle>{t("stats.recentEvents")}</CardTitle>
              <CardAction className="font-mono text-xs text-muted-foreground">
                {t("events.botsCount", { count: formatNumber(query.data.aggregates.bots) })}
              </CardAction>
            </CardHeader>
            <RecentEventsTable events={query.data.recent} />
          </Card>
        </>
      )}
    </div>
  );
}

function StatsSkeleton() {
  return (
    <div className="flex flex-col gap-4" aria-hidden="true">
      <div className="grid grid-cols-2 lg:grid-cols-4 gap-3.5">
        {Array.from({ length: 4 }).map((_, i) => (
          <Skeleton key={i} className="h-20 w-full" />
        ))}
      </div>
      <ChartsSkeleton />
      <Skeleton className="h-48 w-full" />
    </div>
  );
}

/** Chart-grid placeholder shown while the lazy recharts chunk loads. */
function ChartsSkeleton() {
  return (
    <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3" aria-hidden="true">
      {Array.from({ length: 3 }).map((_, i) => (
        <Skeleton key={i} className="h-64 w-full" />
      ))}
    </div>
  );
}
