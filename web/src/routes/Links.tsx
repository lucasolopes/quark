import { AlertTriangle, Link2, Loader2, Plus, RotateCw } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
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
import { CreateLinkDialog } from "@/components/CreateLinkDialog";
import { EditLinkDialog } from "@/components/EditLinkDialog";
import { LinkTable } from "@/components/LinkTable";
import { useDebounce } from "@/hooks/useDebounce";
import { ApiError } from "@/lib/api";
import { mutationErrorToast } from "@/lib/mutation-error";
import { useDeleteLink, useLinks } from "@/lib/queries";
import type { Link } from "@/lib/types";

function matches(link: Link, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return (
    link.code.toLowerCase().includes(q) ||
    link.url.toLowerCase().includes(q) ||
    (link.alias?.toLowerCase().includes(q) ?? false)
  );
}

export function Links() {
  const [search, setSearch] = useState("");
  const [createOpen, setCreateOpen] = useState(false);
  const [editingLink, setEditingLink] = useState<Link | null>(null);
  const [deletingLink, setDeletingLink] = useState<Link | null>(null);
  // Modo client-side: ligado pra sempre (pelo resto da sessão) na primeira
  // vez que o servidor responder 501 — ele não sabe buscar e não vale a
  // pena continuar perguntando.
  const [clientMode, setClientMode] = useState(false);
  const query = useLinks(); // lista base — sempre carregada, é a fonte do fallback client-side
  const deleteLink = useDeleteLink();

  const dq = useDebounce(search, 300);
  const serverSearchEnabled = dq !== "" && !clientMode;
  const serverSearch = useLinks(dq, { enabled: serverSearchEnabled });

  useEffect(() => {
    if (serverSearch.error instanceof ApiError && serverSearch.error.status === 501) setClientMode(true);
  }, [serverSearch.error]);

  // A busca client-side filtra só o que já foi carregado nesta sessão — ela
  // não dispara uma nova página nem uma busca no servidor. "Carregar mais"
  // antes de buscar amplia o conjunto sobre o qual a busca filtra.
  const allLinks = useMemo(() => query.data?.pages.flatMap((page) => page.links) ?? [], [query.data]);
  const searchResults = useMemo(
    () => serverSearch.data?.pages.flatMap((page) => page.links) ?? [],
    [serverSearch.data],
  );

  const usingServerSearch = dq !== "" && !clientMode;
  const filtered = useMemo(() => {
    if (dq === "") return allLinks;
    if (clientMode) return allLinks.filter((link) => matches(link, dq));
    return searchResults;
  }, [allLinks, searchResults, clientMode, dq]);

  const activeQuery = usingServerSearch ? serverSearch : query;
  // 501 é o sinal de "sem suporte a busca" e vira `clientMode` (ver efeito
  // acima) — não é uma falha real, então não deve acionar este card. Qualquer
  // outro erro (500, 429, 503…) é uma falha de verdade e não pode ser
  // confundida com "nenhum resultado", ou o usuário é enganado a pensar que
  // buscou e não achou nada.
  const serverSearchFailed =
    usingServerSearch && serverSearch.isError && !(serverSearch.error instanceof ApiError && serverSearch.error.status === 501);

  async function handleConfirmDelete() {
    if (!deletingLink) return;
    try {
      await deleteLink.mutateAsync(deletingLink.code);
      toast.success("Link excluído.");
      setDeletingLink(null);
    } catch (err) {
      mutationErrorToast(err, (e) =>
        e instanceof ApiError && e.status === 429
          ? "Muitas requisições. Tente de novo em um instante."
          : "Não foi possível excluir o link. Tente de novo.",
      );
    }
  }

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="font-heading text-2xl font-semibold">Links</h1>
          <p className="mt-1 text-sm text-muted-foreground">Todos os links curtos criados no sistema.</p>
        </div>
        <Button onClick={() => setCreateOpen(true)}>
          <Plus className="size-4" />
          Criar link
        </Button>
      </div>

      <div className="relative max-w-sm">
        <Input
          type="search"
          placeholder="Buscar por código, URL ou alias…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          aria-label="Buscar links"
        />
        {usingServerSearch && serverSearch.isFetching && (
          <Loader2
            className="absolute right-2 top-1/2 size-4 -translate-y-1/2 animate-spin text-muted-foreground"
            aria-hidden="true"
          />
        )}
      </div>

      {query.isPending && <LinksSkeleton />}

      {query.isError && (
        <Card className="border-destructive/30">
          <CardContent className="flex flex-col items-center gap-3 py-8 text-center">
            <AlertTriangle className="size-8 text-destructive" aria-hidden="true" />
            <div>
              <p className="font-medium">Não foi possível carregar os links.</p>
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

      {!query.isPending && !query.isError && allLinks.length === 0 && (
        <Card>
          <CardContent className="flex flex-col items-center gap-3 py-12 text-center">
            <Link2 className="size-8 text-muted-foreground" aria-hidden="true" />
            <div>
              <p className="font-medium">Nenhum link ainda.</p>
              <p className="text-sm text-muted-foreground">Crie o primeiro link curto para começar.</p>
            </div>
            <Button onClick={() => setCreateOpen(true)}>
              <Plus className="size-4" />
              Criar link
            </Button>
          </CardContent>
        </Card>
      )}

      {!query.isPending && !query.isError && serverSearchFailed && (
        <Card className="border-destructive/30">
          <CardContent className="flex flex-col items-center gap-3 py-8 text-center">
            <AlertTriangle className="size-8 text-destructive" aria-hidden="true" />
            <div>
              <p className="font-medium">Não foi possível buscar.</p>
              <p className="text-sm text-muted-foreground">
                {serverSearch.error instanceof Error ? serverSearch.error.message : "Tente de novo em alguns instantes."}
              </p>
            </div>
            <Button variant="outline" onClick={() => serverSearch.refetch()}>
              <RotateCw className="size-4" />
              Tentar de novo
            </Button>
          </CardContent>
        </Card>
      )}

      {!query.isPending &&
        !query.isError &&
        !serverSearchFailed &&
        allLinks.length > 0 &&
        dq !== "" &&
        !activeQuery.isPending &&
        filtered.length === 0 && (
          <Card>
            <CardContent className="py-8 text-center text-sm text-muted-foreground">
              nenhum link encontrado para "{dq}"
            </CardContent>
          </Card>
        )}

      {!query.isPending && !query.isError && !serverSearchFailed && filtered.length > 0 && (
        <Card className="py-0">
          <LinkTable
            links={filtered}
            onEdit={(link) => setEditingLink(link)}
            onDelete={(link) => setDeletingLink(link)}
          />
        </Card>
      )}

      {activeQuery.hasNextPage && (
        <Button
          variant="outline"
          onClick={() => activeQuery.fetchNextPage()}
          disabled={activeQuery.isFetchingNextPage}
          className="self-center"
        >
          {activeQuery.isFetchingNextPage ? "Carregando…" : "Carregar mais"}
        </Button>
      )}

      <CreateLinkDialog open={createOpen} onOpenChange={setCreateOpen} />

      {editingLink && (
        <EditLinkDialog
          key={editingLink.code}
          link={editingLink}
          open
          onOpenChange={(open) => !open && setEditingLink(null)}
        />
      )}

      <AlertDialog open={deletingLink != null} onOpenChange={(open) => !open && setDeletingLink(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Excluir {deletingLink?.code}?</AlertDialogTitle>
            <AlertDialogDescription>
              Isto não pode ser desfeito. O link deixará de redirecionar imediatamente.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={deleteLink.isPending}>Cancelar</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              disabled={deleteLink.isPending}
              onClick={handleConfirmDelete}
            >
              {deleteLink.isPending ? "Excluindo…" : "Excluir"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

function LinksSkeleton() {
  return (
    <div className="flex flex-col gap-2" aria-hidden="true">
      {Array.from({ length: 5 }).map((_, i) => (
        <Skeleton key={i} className="h-10 w-full" />
      ))}
    </div>
  );
}
