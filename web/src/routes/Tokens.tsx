import { AlertTriangle, KeyRound, Plus, RotateCw } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { CreateTokenDialog } from "@/components/CreateTokenDialog";
import { PageHeader } from "@/components/PageHeader";
import { useT, type MessageKey } from "@/i18n";
import { formatDate } from "@/lib/format";
import { ApiError } from "@/lib/api";
import { mutationErrorToast } from "@/lib/mutation-error";
import { useDeleteToken, useTokens } from "@/lib/queries";
import type { ApiToken, Scope } from "@/lib/types";

/** Message key (under `tokens.scope`) for each scope's display label. */
const SCOPE_LABEL_KEY: Record<Scope, MessageKey> = {
  links_read: "tokens.scope.linksRead",
  links_write: "tokens.scope.linksWrite",
  webhooks: "tokens.scope.webhooks",
  analytics: "tokens.scope.analytics",
  full: "tokens.scope.full",
};

/** Friendly error message for revoke (429/generic). */
function revokeErrorMessage(err: unknown, t: (key: MessageKey) => string): string {
  if (err instanceof ApiError && err.status === 429) return t("common.rateLimited");
  return t("tokens.revokeGenericError");
}

export function Tokens() {
  const t = useT();
  const [createOpen, setCreateOpen] = useState(false);
  const [revokingToken, setRevokingToken] = useState<ApiToken | null>(null);
  const query = useTokens();
  const deleteToken = useDeleteToken();

  const tokens = query.data?.tokens ?? [];

  async function handleConfirmRevoke() {
    if (!revokingToken) return;
    try {
      await deleteToken.mutateAsync(revokingToken.id);
      toast.success(t("tokens.revokedSuccess"));
      setRevokingToken(null);
    } catch (err) {
      mutationErrorToast(err, (e) => revokeErrorMessage(e, t));
    }
  }

  return (
    <div className="flex flex-col gap-4 animate-rise max-w-[860px]">
      <PageHeader
        title={t("tokens.heading")}
        subtitle={t("tokens.subtitle")}
        actions={
          <Button onClick={() => setCreateOpen(true)}>
            <Plus className="size-4" />
            {t("tokens.createButton")}
          </Button>
        }
      />

      {query.isPending && <TokensSkeleton />}

      {query.isError && (
        <Card className="border-destructive/30">
          <CardContent className="flex flex-col items-center gap-3 py-8 text-center">
            <AlertTriangle className="size-8 text-destructive" aria-hidden="true" />
            <div>
              <p className="font-medium">{t("tokens.loadError")}</p>
              <p className="text-sm text-muted-foreground">
                {query.error instanceof Error ? query.error.message : t("common.retryHint")}
              </p>
            </div>
            <Button variant="outline" onClick={() => query.refetch()}>
              <RotateCw className="size-4" />
              {t("common.retry")}
            </Button>
          </CardContent>
        </Card>
      )}

      {!query.isPending && !query.isError && tokens.length === 0 && (
        <Card>
          <CardContent className="flex flex-col items-center gap-3 py-12 text-center">
            <div className="flex size-10 items-center justify-center rounded-[9px] bg-secondary">
              <KeyRound className="size-[18px] text-muted-foreground" aria-hidden="true" />
            </div>
            <div>
              <p className="font-medium">{t("tokens.emptyTitle")}</p>
              <p className="text-sm text-muted-foreground">{t("tokens.emptySubtitle")}</p>
            </div>
            <Button onClick={() => setCreateOpen(true)}>
              <Plus className="size-4" />
              {t("tokens.createButton")}
            </Button>
          </CardContent>
        </Card>
      )}

      {!query.isPending && !query.isError && tokens.length > 0 && (
        <ul className="flex flex-col gap-2.5">
          {tokens.map((token) => (
            <li
              key={token.id}
              data-testid="token-card"
              className="card-hover flex flex-col gap-3 rounded-lg border border-border bg-card p-4 shadow-card"
            >
              <div className="flex flex-wrap items-center justify-between gap-3">
                <span className="min-w-0 truncate text-[14.5px] font-semibold text-strong">{token.name}</span>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  className="shrink-0 text-destructive hover:text-destructive"
                  aria-label={t("tokens.revokeAria", { name: token.name })}
                  onClick={() => setRevokingToken(token)}
                >
                  {t("tokens.revoke")}
                </Button>
              </div>
              <div className="flex flex-wrap items-center gap-2">
                {token.scopes.map((scope) => (
                  <span
                    key={scope}
                    className="rounded-md bg-secondary px-2 py-0.5 font-mono text-[11.5px] text-foreground"
                  >
                    {t(SCOPE_LABEL_KEY[scope])}
                  </span>
                ))}
                <span className="text-xs text-muted-foreground">
                  ·{" "}
                  {token.rate_limit_per_min == null
                    ? t("tokens.noRateLimit")
                    : t("tokens.perMinute", { rate: token.rate_limit_per_min })}
                </span>
                <span className="text-xs text-muted-foreground">· {formatDate(token.created)}</span>
              </div>
            </li>
          ))}
        </ul>
      )}

      <CreateTokenDialog open={createOpen} onOpenChange={setCreateOpen} />

      <AlertDialog open={revokingToken != null} onOpenChange={(open) => !open && setRevokingToken(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("tokens.revokeTitle", { name: revokingToken?.name ?? "" })}</AlertDialogTitle>
            <AlertDialogDescription>{t("tokens.revokeDescription")}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={deleteToken.isPending}>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              disabled={deleteToken.isPending}
              onClick={handleConfirmRevoke}
            >
              {deleteToken.isPending ? t("tokens.revoking") : t("tokens.revoke")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

function TokensSkeleton() {
  return (
    <div className="flex flex-col gap-2.5" aria-hidden="true">
      {Array.from({ length: 4 }).map((_, i) => (
        <Skeleton key={i} className="h-[72px] w-full" />
      ))}
    </div>
  );
}
