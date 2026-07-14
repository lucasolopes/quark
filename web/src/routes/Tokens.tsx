import { AlertTriangle, KeyRound, Plus, RotateCw, Trash2 } from "lucide-react";
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
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { CreateTokenDialog } from "@/components/CreateTokenDialog";
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
  blocklist: "tokens.scope.blocklist",
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
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="font-heading text-2xl font-semibold">{t("tokens.heading")}</h1>
          <p className="mt-1 text-sm text-muted-foreground">{t("tokens.subtitle")}</p>
        </div>
        <Button onClick={() => setCreateOpen(true)}>
          <Plus className="size-4" />
          {t("tokens.createButton")}
        </Button>
      </div>

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
            <KeyRound className="size-8 text-muted-foreground" aria-hidden="true" />
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
        <Card className="py-0">
          <ul className="divide-y">
            {tokens.map((token) => (
              <li key={token.id} className="flex flex-wrap items-center justify-between gap-3 px-4 py-3">
                <div className="flex min-w-0 flex-col gap-1.5">
                  <span className="font-medium">{token.name}</span>
                  <div className="flex flex-wrap items-center gap-1.5">
                    {token.scopes.map((scope) => (
                      <Badge key={scope} variant="secondary">
                        {t(SCOPE_LABEL_KEY[scope])}
                      </Badge>
                    ))}
                  </div>
                  <span className="text-xs text-muted-foreground">
                    {token.rate_limit_per_min == null
                      ? t("tokens.noRateLimit")
                      : t("tokens.perMinute", { rate: token.rate_limit_per_min })}
                    {" · "}
                    {formatDate(token.created)}
                  </span>
                </div>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  aria-label={t("tokens.revokeAria", { name: token.name })}
                  onClick={() => setRevokingToken(token)}
                >
                  <Trash2 className="size-4" />
                  {t("tokens.revoke")}
                </Button>
              </li>
            ))}
          </ul>
        </Card>
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
    <div className="flex flex-col gap-2" aria-hidden="true">
      {Array.from({ length: 4 }).map((_, i) => (
        <Skeleton key={i} className="h-14 w-full" />
      ))}
    </div>
  );
}
