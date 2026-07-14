import { AlertTriangle, Plus, RotateCw, ShieldOff, Trash2 } from "lucide-react";
import { useState, type FormEvent } from "react";
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
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { useT, type MessageKey } from "@/i18n";
import { ApiError } from "@/lib/api";
import { mutationErrorToast } from "@/lib/mutation-error";
import { useAddBlocked, useBlocklist, useRemoveBlocked } from "@/lib/queries";

/** Friendly error message for the blocklist mutations (add/remove). */
function mutationErrorMessage(err: unknown, fallbackKey: MessageKey, t: (key: MessageKey) => string): string {
  if (err instanceof ApiError && err.status === 429) return t("common.rateLimited");
  return t(fallbackKey);
}

export function Blocklist() {
  const t = useT();
  const [domain, setDomain] = useState("");
  const [removingDomain, setRemovingDomain] = useState<string | null>(null);
  const query = useBlocklist();
  const addBlocked = useAddBlocked();
  const removeBlocked = useRemoveBlocked();

  const domains = query.data?.domains ?? [];

  async function handleAdd(e: FormEvent) {
    e.preventDefault();
    const trimmed = domain.trim();
    if (!trimmed) return;
    try {
      await addBlocked.mutateAsync(trimmed);
      toast.success(t("blocklist.blockedSuccess", { domain: trimmed }));
      setDomain("");
    } catch (err) {
      mutationErrorToast(err, (e) => mutationErrorMessage(e, "blocklist.addGenericError", t));
    }
  }

  async function handleConfirmRemove() {
    if (!removingDomain) return;
    try {
      await removeBlocked.mutateAsync(removingDomain);
      toast.success(t("blocklist.unblockedSuccess", { domain: removingDomain }));
      setRemovingDomain(null);
    } catch (err) {
      mutationErrorToast(err, (e) => mutationErrorMessage(e, "blocklist.removeGenericError", t));
    }
  }

  return (
    <div className="flex flex-col gap-4">
      <div>
        <h1 className="font-heading text-2xl font-semibold">{t("blocklist.heading")}</h1>
        <p className="mt-1 text-sm text-muted-foreground">{t("blocklist.subtitle")}</p>
      </div>

      <form onSubmit={handleAdd} className="flex flex-wrap items-center gap-2">
        <Input
          type="text"
          placeholder={t("blocklist.domainPlaceholder")}
          value={domain}
          onChange={(e) => setDomain(e.target.value)}
          aria-label={t("blocklist.domainAriaLabel")}
          className="max-w-sm"
        />
        <Button type="submit" disabled={addBlocked.isPending || !domain.trim()}>
          <Plus className="size-4" />
          {addBlocked.isPending ? t("blocklist.adding") : t("blocklist.add")}
        </Button>
      </form>

      {query.isPending && <BlocklistSkeleton />}

      {query.isError && (
        <Card className="border-destructive/30">
          <CardContent className="flex flex-col items-center gap-3 py-8 text-center">
            <AlertTriangle className="size-8 text-destructive" aria-hidden="true" />
            <div>
              <p className="font-medium">{t("blocklist.loadError")}</p>
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

      {!query.isPending && !query.isError && domains.length === 0 && (
        <Card>
          <CardContent className="flex flex-col items-center gap-3 py-12 text-center">
            <ShieldOff className="size-8 text-muted-foreground" aria-hidden="true" />
            <div>
              <p className="font-medium">{t("blocklist.emptyTitle")}</p>
              <p className="text-sm text-muted-foreground">{t("blocklist.emptySubtitle")}</p>
            </div>
          </CardContent>
        </Card>
      )}

      {!query.isPending && !query.isError && domains.length > 0 && (
        <Card className="py-0">
          <ul className="divide-y">
            {domains.map((d) => (
              <li key={d} className="flex items-center justify-between gap-3 px-4 py-3">
                <span className="truncate font-mono text-sm">{d}</span>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  aria-label={t("blocklist.removeAria", { domain: d })}
                  onClick={() => setRemovingDomain(d)}
                >
                  <Trash2 className="size-4" />
                  {t("blocklist.remove")}
                </Button>
              </li>
            ))}
          </ul>
        </Card>
      )}

      <AlertDialog open={removingDomain != null} onOpenChange={(open) => !open && setRemovingDomain(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("blocklist.removeTitle", { domain: removingDomain ?? "" })}</AlertDialogTitle>
            <AlertDialogDescription>{t("blocklist.removeDescription")}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={removeBlocked.isPending}>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              disabled={removeBlocked.isPending}
              onClick={handleConfirmRemove}
            >
              {removeBlocked.isPending ? t("blocklist.removing") : t("blocklist.remove")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

function BlocklistSkeleton() {
  return (
    <div className="flex flex-col gap-2" aria-hidden="true">
      {Array.from({ length: 4 }).map((_, i) => (
        <Skeleton key={i} className="h-10 w-full" />
      ))}
    </div>
  );
}
