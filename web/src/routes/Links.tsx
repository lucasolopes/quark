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
import { useT } from "@/i18n";
import { ApiError } from "@/lib/api";
import { mutationErrorToast } from "@/lib/mutation-error";
import { useDeleteLink, useFolders, useLinks, useTags } from "@/lib/queries";
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
  const t = useT();
  const [search, setSearch] = useState("");
  const [tag, setTag] = useState("");
  const [folder, setFolder] = useState("");
  const [brokenOnly, setBrokenOnly] = useState(false);
  const [createOpen, setCreateOpen] = useState(false);
  const [editingLink, setEditingLink] = useState<Link | null>(null);
  const [deletingLink, setDeletingLink] = useState<Link | null>(null);
  const [clientMode, setClientMode] = useState(false);
  const health = brokenOnly ? "broken" : undefined;
  const query = useLinks(undefined, tag || undefined, folder || undefined, health);
  const deleteLink = useDeleteLink();
  const tagsQuery = useTags();
  const foldersQuery = useFolders();

  const dq = useDebounce(search, 300);
  const serverSearchEnabled = dq !== "" && !clientMode;
  const serverSearch = useLinks(dq, tag || undefined, folder || undefined, health, { enabled: serverSearchEnabled });

  useEffect(() => {
    if (serverSearch.error instanceof ApiError && serverSearch.error.status === 501) setClientMode(true);
  }, [serverSearch.error]);

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
  const serverSearchFailed =
    usingServerSearch && serverSearch.isError && !(serverSearch.error instanceof ApiError && serverSearch.error.status === 501);

  async function handleConfirmDelete() {
    if (!deletingLink) return;
    try {
      await deleteLink.mutateAsync(deletingLink.code);
      toast.success(t("links.deleteSuccess"));
      setDeletingLink(null);
    } catch (err) {
      mutationErrorToast(err, (e) =>
        e instanceof ApiError && e.status === 429 ? t("common.rateLimited") : t("links.deleteGenericError"),
      );
    }
  }

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="font-heading text-2xl font-semibold">{t("links.heading")}</h1>
          <p className="mt-1 text-sm text-muted-foreground">{t("links.subtitle")}</p>
        </div>
        <Button onClick={() => setCreateOpen(true)}>
          <Plus className="size-4" />
          {t("links.createButton")}
        </Button>
      </div>

      <div className="flex flex-wrap items-center gap-3">
        <div className="relative max-w-sm grow">
          <Input
            type="search"
            placeholder={t("links.searchPlaceholder")}
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            aria-label={t("links.searchAriaLabel")}
          />
          {usingServerSearch && serverSearch.isFetching && (
            <Loader2
              className="absolute right-2 top-1/2 size-4 -translate-y-1/2 animate-spin text-muted-foreground"
              aria-hidden="true"
            />
          )}
        </div>

        <select
          value={tag}
          onChange={(e) => setTag(e.target.value)}
          aria-label={t("links.tagFilterLabel")}
          className="h-9 rounded-md border border-input bg-transparent px-3 text-sm shadow-xs outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50"
        >
          <option value="">{t("links.tagFilterAllOption")}</option>
          {(tagsQuery.data?.tags ?? []).map((tagOption) => (
            <option key={tagOption.name} value={tagOption.name}>
              {t("links.tagFilterOption", { name: tagOption.name, count: tagOption.count })}
            </option>
          ))}
        </select>

        <select
          value={folder}
          onChange={(e) => setFolder(e.target.value)}
          aria-label={t("links.folderFilterLabel")}
          className="h-9 rounded-md border border-input bg-transparent px-3 text-sm shadow-xs outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50"
        >
          <option value="">{t("links.folderFilterAll")}</option>
          {(foldersQuery.data?.folders ?? []).map((folderOption) => (
            <option key={folderOption.name} value={folderOption.name}>
              {t("links.folderFilterOption", { name: folderOption.name, count: folderOption.count })}
            </option>
          ))}
        </select>

        <label className="flex h-9 items-center gap-2 text-sm text-muted-foreground">
          <input
            type="checkbox"
            className="size-4 rounded border-input accent-primary"
            checked={brokenOnly}
            onChange={(e) => setBrokenOnly(e.target.checked)}
          />
          {t("links.brokenFilterLabel")}
        </label>
      </div>

      {query.isPending && <LinksSkeleton />}

      {query.isError && (
        <Card className="border-destructive/30">
          <CardContent className="flex flex-col items-center gap-3 py-8 text-center">
            <AlertTriangle className="size-8 text-destructive" aria-hidden="true" />
            <div>
              <p className="font-medium">{t("links.loadError")}</p>
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

      {!query.isPending && !query.isError && allLinks.length === 0 && (
        <Card>
          <CardContent className="flex flex-col items-center gap-3 py-12 text-center">
            <Link2 className="size-8 text-muted-foreground" aria-hidden="true" />
            <div>
              <p className="font-medium">{t("links.emptyTitle")}</p>
              <p className="text-sm text-muted-foreground">{t("links.emptySubtitle")}</p>
            </div>
            <Button onClick={() => setCreateOpen(true)}>
              <Plus className="size-4" />
              {t("links.createButton")}
            </Button>
          </CardContent>
        </Card>
      )}

      {!query.isPending && !query.isError && serverSearchFailed && (
        <Card className="border-destructive/30">
          <CardContent className="flex flex-col items-center gap-3 py-8 text-center">
            <AlertTriangle className="size-8 text-destructive" aria-hidden="true" />
            <div>
              <p className="font-medium">{t("links.searchError")}</p>
              <p className="text-sm text-muted-foreground">
                {serverSearch.error instanceof Error ? serverSearch.error.message : t("common.retryHint")}
              </p>
            </div>
            <Button variant="outline" onClick={() => serverSearch.refetch()}>
              <RotateCw className="size-4" />
              {t("common.retry")}
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
              {t("links.noResults", { term: dq })}
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
          {activeQuery.isFetchingNextPage ? t("common.loadingMore") : t("common.loadMore")}
        </Button>
      )}

      <CreateLinkDialog
        open={createOpen}
        onOpenChange={setCreateOpen}
        folders={foldersQuery.data?.folders ?? []}
      />

      {editingLink && (
        <EditLinkDialog
          key={editingLink.code}
          link={editingLink}
          open
          onOpenChange={(open) => !open && setEditingLink(null)}
          folders={foldersQuery.data?.folders ?? []}
        />
      )}

      <AlertDialog open={deletingLink != null} onOpenChange={(open) => !open && setDeletingLink(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("links.deleteTitle", { code: deletingLink?.code ?? "" })}</AlertDialogTitle>
            <AlertDialogDescription>{t("links.deleteDescription")}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={deleteLink.isPending}>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              disabled={deleteLink.isPending}
              onClick={handleConfirmDelete}
            >
              {deleteLink.isPending ? t("links.deleting") : t("links.delete")}
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
