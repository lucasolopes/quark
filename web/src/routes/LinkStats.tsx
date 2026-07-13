import { AlertTriangle, ArrowLeft, MousePointerClick, RotateCw, Timer, TimerReset } from "lucide-react";
import type { ReactNode } from "react";
import { Link, useParams } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { RecentEventsTable } from "@/components/RecentEventsTable";
import { StatsCharts } from "@/components/StatsCharts";
import { formatDateTime } from "@/lib/format";
import { useStats } from "@/lib/queries";

export function LinkStats() {
  const { code = "" } = useParams<{ code: string }>();
  const query = useStats(code);

  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center gap-3">
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="Voltar para Links"
          render={<Link to="/links" />}
        >
          <ArrowLeft className="size-4" />
        </Button>
        <div>
          <h1 className="font-heading text-2xl font-semibold">Estatísticas</h1>
          <p className="mt-1 text-sm text-muted-foreground">
            Cliques do link <span className="font-mono">{code}</span>.
          </p>
        </div>
      </div>

      {query.isPending && <StatsSkeleton />}

      {query.isError && (
        <Card className="border-destructive/30">
          <CardContent className="flex flex-col items-center gap-3 py-8 text-center">
            <AlertTriangle className="size-8 text-destructive" aria-hidden="true" />
            <div>
              <p className="font-medium">Não foi possível carregar as estatísticas.</p>
              <p className="text-sm text-muted-foreground">
                {query.error instanceof Error ? query.error.message : "Tente de novo em alguns instantes."}
              </p>
            </div>
            <div className="flex gap-2">
              <Button variant="outline" onClick={() => query.refetch()}>
                <RotateCw className="size-4" />
                Tentar de novo
              </Button>
              <Button variant="outline" render={<Link to="/links" />}>
                Voltar para Links
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
              <p className="font-medium">Sem cliques ainda.</p>
              <p className="text-sm text-muted-foreground">
                Quando alguém abrir este link, os cliques aparecem aqui.
              </p>
            </div>
          </CardContent>
        </Card>
      )}

      {query.isSuccess && query.data.aggregates.total > 0 && (
        <>
          <div className="grid gap-4 sm:grid-cols-3">
            <StatCard
              icon={<MousePointerClick className="size-4" aria-hidden="true" />}
              label="Total de cliques"
              value={String(query.data.aggregates.total)}
            />
            <StatCard
              icon={<Timer className="size-4" aria-hidden="true" />}
              label="Primeiro clique"
              value={formatDateTime(query.data.aggregates.first_ts)}
            />
            <StatCard
              icon={<TimerReset className="size-4" aria-hidden="true" />}
              label="Último clique"
              value={formatDateTime(query.data.aggregates.last_ts)}
            />
          </div>

          <StatsCharts aggregates={query.data.aggregates} />

          <Card className="py-0">
            <CardHeader className="pt-4">
              <CardTitle>Eventos recentes</CardTitle>
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
}

function StatCard({ icon, label, value }: StatCardProps) {
  return (
    <Card>
      <CardContent className="flex items-center gap-3">
        <div className="flex size-9 shrink-0 items-center justify-center rounded-full bg-muted text-muted-foreground">
          {icon}
        </div>
        <div>
          <p className="text-sm text-muted-foreground">{label}</p>
          <p className="font-heading text-xl font-semibold">{value}</p>
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
      <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
        {Array.from({ length: 3 }).map((_, i) => (
          <Skeleton key={i} className="h-64 w-full" />
        ))}
      </div>
      <Skeleton className="h-48 w-full" />
    </div>
  );
}
