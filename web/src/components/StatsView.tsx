import { AlertTriangle, Bot, MousePointerClick, RotateCw, Timer, TimerReset } from "lucide-react";
import { lazy, Suspense, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { RecentEventsTable } from "@/components/RecentEventsTable";
import { useT } from "@/i18n";

// recharts is heavy; keep it out of the main bundle by loading the charts
// component only when a link's stats are actually rendered.
const StatsCharts = lazy(() => import("@/components/StatsCharts").then((m) => ({ default: m.StatsCharts })));
import { formatDateTime, formatNumber } from "@/lib/format";
import { useStats } from "@/lib/queries";
import { cn } from "@/lib/utils";

export function StatsView({ code }: { code: string }) {
  const t = useT();
  const query = useStats(code);

  return (
    <div className="flex flex-col gap-4">
      <div>
        <h1 className="font-heading text-2xl font-semibold">{t("stats.heading")}</h1>
        <p className="mt-1 text-sm text-muted-foreground">{t("stats.subtitle", { code })}</p>
      </div>

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
          <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
            <StatCard
              icon={<MousePointerClick className="size-4" aria-hidden="true" />}
              label={t("stats.totalClicks")}
              value={formatNumber(query.data.aggregates.total)}
              accent
            />
            <StatCard
              icon={<Bot className="size-4" aria-hidden="true" />}
              label={t("stats.botsExcluded")}
              value={formatNumber(query.data.aggregates.bots)}
            />
            <StatCard
              icon={<Timer className="size-4" aria-hidden="true" />}
              label={t("stats.firstClick")}
              value={formatDateTime(query.data.aggregates.first_ts)}
            />
            <StatCard
              icon={<TimerReset className="size-4" aria-hidden="true" />}
              label={t("stats.lastClick")}
              value={formatDateTime(query.data.aggregates.last_ts)}
            />
          </div>

          <p className="text-sm text-muted-foreground">{t("stats.chartsHumanOnlyHint")}</p>

          <Suspense fallback={<ChartsSkeleton />}>
            <StatsCharts aggregates={query.data.aggregates} />
          </Suspense>

          <Card className="py-0">
            <CardHeader className="pt-4">
              <CardTitle>{t("stats.recentEvents")}</CardTitle>
            </CardHeader>
            <RecentEventsTable events={query.data.recent} />
          </Card>
        </>
      )}
    </div>
  );
}

interface StatCardProps {
  icon: ReactNode;
  label: string;
  value: string;
  /** Marks the hero metric (headline number) — large Space Grotesk in plasma-lime. */
  accent?: boolean;
}

function StatCard({ icon, label, value, accent = false }: StatCardProps) {
  return (
    <Card>
      <CardContent className="flex items-center gap-3">
        <div
          className={cn(
            "flex size-9 shrink-0 items-center justify-center rounded-full",
            accent ? "bg-primary/12 text-brand-ink" : "bg-muted text-muted-foreground",
          )}
        >
          {icon}
        </div>
        <div className="min-w-0">
          <p className="font-mono text-[11px] tracking-[0.08em] text-muted-foreground uppercase">{label}</p>
          <p
            className={cn(
              "font-heading font-bold tracking-tight tabular-nums",
              accent ? "text-3xl text-brand-ink" : "text-xl",
            )}
          >
            {value}
          </p>
        </div>
      </CardContent>
    </Card>
  );
}

function StatsSkeleton() {
  return (
    <div className="flex flex-col gap-4" aria-hidden="true">
      <div className="grid gap-4 sm:grid-cols-3">
        {Array.from({ length: 3 }).map((_, i) => (
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
