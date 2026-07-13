import { AlertTriangle, Link2, Plus, RotateCw } from "lucide-react";
import { useMemo, useState } from "react";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { LinkTable } from "@/components/LinkTable";
import { useLinks } from "@/lib/queries";
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
  const [stub, setStub] = useState<{ action: "criar" | "editar" | "excluir"; link?: Link } | null>(null);
  const query = useLinks();

  // A busca filtra só o que já foi carregado nesta sessão — ela não dispara
  // uma nova página nem uma busca no servidor. "Carregar mais" antes de
  // buscar amplia o conjunto sobre o qual a busca filtra.
  const allLinks = useMemo(() => query.data?.pages.flatMap((page) => page.links) ?? [], [query.data]);
  const filtered = useMemo(() => allLinks.filter((link) => matches(link, search)), [allLinks, search]);

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="font-heading text-2xl font-semibold">Links</h1>
          <p className="mt-1 text-sm text-muted-foreground">Todos os links curtos criados no sistema.</p>
        </div>
        <Button onClick={() => setStub({ action: "criar" })}>
          <Plus className="size-4" />
          Criar link
        </Button>
      </div>

      <Input
        type="search"
        placeholder="Buscar por código, URL ou alias…"
        value={search}
        onChange={(e) => setSearch(e.target.value)}
        aria-label="Buscar links"
        className="max-w-sm"
      />

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
            <Button onClick={() => setStub({ action: "criar" })}>
              <Plus className="size-4" />
              Criar link
            </Button>
          </CardContent>
        </Card>
      )}

      {!query.isPending && !query.isError && allLinks.length > 0 && filtered.length === 0 && (
        <Card>
          <CardContent className="py-8 text-center text-sm text-muted-foreground">
            Nenhum link corresponde a "{search}".
          </CardContent>
        </Card>
      )}

      {!query.isPending && !query.isError && filtered.length > 0 && (
        <Card className="py-0">
          <LinkTable
            links={filtered}
            onEdit={(link) => setStub({ action: "editar", link })}
            onDelete={(link) => setStub({ action: "excluir", link })}
          />
        </Card>
      )}

      {query.hasNextPage && !search && (
        <Button variant="outline" onClick={() => query.fetchNextPage()} disabled={query.isFetchingNextPage} className="self-center">
          {query.isFetchingNextPage ? "Carregando…" : "Carregar mais"}
        </Button>
      )}

      <Dialog open={stub != null} onOpenChange={(open) => !open && setStub(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {stub?.action === "criar" && "Criar link"}
              {stub?.action === "editar" && `Editar ${stub.link?.code}`}
              {stub?.action === "excluir" && `Excluir ${stub.link?.code}`}
            </DialogTitle>
            <DialogDescription>Essa ação chega na próxima etapa.</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setStub(null)}>
              Fechar
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
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
