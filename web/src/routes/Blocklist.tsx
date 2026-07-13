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
import { ApiError } from "@/lib/api";
import { mutationErrorToast } from "@/lib/mutation-error";
import { useAddBlocked, useBlocklist, useRemoveBlocked } from "@/lib/queries";

/** Mensagem de erro amigável para as mutações de blocklist (adicionar/remover). */
function mutationErrorMessage(err: unknown, fallback: string): string {
  if (err instanceof ApiError && err.status === 429) {
    return "Muitas requisições. Tente de novo em um instante.";
  }
  return fallback;
}

export function Blocklist() {
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
      toast.success(`${trimmed} bloqueado.`);
      setDomain("");
    } catch (err) {
      mutationErrorToast(err, (e) => mutationErrorMessage(e, "Não foi possível bloquear o domínio. Tente de novo."));
    }
  }

  async function handleConfirmRemove() {
    if (!removingDomain) return;
    try {
      await removeBlocked.mutateAsync(removingDomain);
      toast.success(`${removingDomain} desbloqueado.`);
      setRemovingDomain(null);
    } catch (err) {
      mutationErrorToast(err, (e) => mutationErrorMessage(e, "Não foi possível desbloquear o domínio. Tente de novo."));
    }
  }

  return (
    <div className="flex flex-col gap-4">
      <div>
        <h1 className="font-heading text-2xl font-semibold">Blocklist</h1>
        <p className="mt-1 text-sm text-muted-foreground">
          Domínios impedidos de serem usados como destino de um link curto.
        </p>
      </div>

      <form onSubmit={handleAdd} className="flex flex-wrap items-center gap-2">
        <Input
          type="text"
          placeholder="dominio-suspeito.com"
          value={domain}
          onChange={(e) => setDomain(e.target.value)}
          aria-label="Domínio a bloquear"
          className="max-w-sm"
        />
        <Button type="submit" disabled={addBlocked.isPending || !domain.trim()}>
          <Plus className="size-4" />
          {addBlocked.isPending ? "Adicionando…" : "Adicionar"}
        </Button>
      </form>

      {query.isPending && <BlocklistSkeleton />}

      {query.isError && (
        <Card className="border-destructive/30">
          <CardContent className="flex flex-col items-center gap-3 py-8 text-center">
            <AlertTriangle className="size-8 text-destructive" aria-hidden="true" />
            <div>
              <p className="font-medium">Não foi possível carregar a blocklist.</p>
              <p className="text-sm text-muted-foreground">
                {query.error instanceof Error ? query.error.message : "Tente de novo em alguns instantes."}
              </p>
            </div>
            <Button variant="outline" onClick={() => query.refetch()}>
              <RotateCw className="size-4" />
              Tentar de novo
            </Button>
          </CardContent>
        </Card>
      )}

      {!query.isPending && !query.isError && domains.length === 0 && (
        <Card>
          <CardContent className="flex flex-col items-center gap-3 py-12 text-center">
            <ShieldOff className="size-8 text-muted-foreground" aria-hidden="true" />
            <div>
              <p className="font-medium">Nenhum domínio bloqueado.</p>
              <p className="text-sm text-muted-foreground">
                Domínios adicionados aqui não poderão ser usados como destino de novos links.
              </p>
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
                  aria-label={`Remover ${d} da blocklist`}
                  onClick={() => setRemovingDomain(d)}
                >
                  <Trash2 className="size-4" />
                  Remover
                </Button>
              </li>
            ))}
          </ul>
        </Card>
      )}

      <AlertDialog open={removingDomain != null} onOpenChange={(open) => !open && setRemovingDomain(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Remover {removingDomain} da blocklist?</AlertDialogTitle>
            <AlertDialogDescription>
              Depois de removido, o domínio volta a ser aceito como destino de novos links.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={removeBlocked.isPending}>Cancelar</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              disabled={removeBlocked.isPending}
              onClick={handleConfirmRemove}
            >
              {removeBlocked.isPending ? "Removendo…" : "Remover"}
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
