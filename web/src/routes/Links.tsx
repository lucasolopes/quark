import { AlertTriangle, Link2, Plus, RotateCw } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useSearchParams } from "react-router-dom";
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
import { CreateLinkDialog } from "@/components/CreateLinkDialog";
import { EditLinkDialog } from "@/components/EditLinkDialog";
import { LinkTable } from "@/components/LinkTable";
import { PageHeader } from "@/components/PageHeader";
import { useDebounce } from "@/hooks/useDebounce";
import { useT } from "@/i18n";
import { ApiError } from "@/lib/api";
import { mutationErrorToast } from "@/lib/mutation-error";
import { useDeleteLink, useFolders, useLinks, useTags } from "@/lib/queries";
import { useScopes } from "@/lib/scopes";
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

/** Pill-chip classes for the tag filter (mock isTabLinks.html): active = wash+lime, inactive = muted outline. */
function chipClass(active: boolean): string {
  const base =
    "inline-flex items-center gap-1.5 rounded-full border px-3 py-1.5 text-sm font-medium transition-colors";
  const state = active
    ? "bg-accent-wash border-accent-chip text-brand-ink"
    : "border-border text-muted-foreground hover:text-foreground";
  return `${base} ${state}`;
}

/** Tag chips beyond this count collapse behind a "+N" toggle so a tag-heavy account doesn't push the filter row into a wall of chips. */
const TAG_CHIPS_VISIBLE_CAP = 10;

export function Links() {
  const t = useT();
  const [searchParams, setSearchParams] = useSearchParams();
  const qParam = searchParams.get("q") ?? "";
  const [search, setSearch] = useState(qParam);
  const [tag, setTag] = useState("");
  const [folder, setFolder] = useState("");
  const [brokenOnly, setBrokenOnly] = useState(false);
  const [activeOnly, setActiveOnly] = useState(false);
  const [showAllTags, setShowAllTags] = useState(false);
  const [createOpen, setCreateOpen] = useState(false);
  const [editingLink, setEditingLink] = useState<Link | null>(null);
  const [deletingLink, setDeletingLink] = useState<Link | null>(null);
  const [clientMode, setClientMode] = useState(false);
  const health = brokenOnly ? "broken" : undefined;
  const status = activeOnly ? "active" : undefined;
  const query = useLinks(undefined, tag || undefined, folder || undefined, health, status);
  const deleteLink = useDeleteLink();
  const tagsQuery = useTags();
  const foldersQuery = useFolders();
  // Viewers have no `links_write`: hide every write affordance (create/edit/
  // delete) so the page offers only what they can actually do. The backend
  // enforces this regardless; this just stops the UI from promising a 403.
  const { has } = useScopes();
  const canWrite = has("links_write");

  const dq = useDebounce(search, 300);
  const serverSearchEnabled = dq !== "" && !clientMode;
  const serverSearch = useLinks(dq, tag || undefined, folder || undefined, health, status, { enabled: serverSearchEnabled });

  useEffect(() => {
    if (serverSearch.error instanceof ApiError && serverSearch.error.status === 501) setClientMode(true);
  }, [serverSearch.error]);

  // The topbar (Shell.tsx) is the ONE search entry point for the app (mock:
  // appShell.html; isTabLinks.html has no local search) — this screen has no
  // search input of its own. `?q=` is the sole source of truth for the term;
  // mirror it into local state (including back to "") so it keeps feeding the
  // debounce/server-search below, whether the param changed via topbar typing,
  // an Enter-navigation from elsewhere, or the browser's back/forward.
  useEffect(() => {
    setSearch(qParam);
  }, [qParam]);

  // Arriving via `?new=1` (topbar "New link") opens the create dialog once, then
  // strips the param so closing the dialog or a refresh does not reopen it.
  // If the user is a viewer (no links_write), strip the param without opening.
  useEffect(() => {
    if (searchParams.get("new") !== "1") return;
    if (canWrite) {
      setCreateOpen(true);
    }
    const next = new URLSearchParams(searchParams);
    next.delete("new");
    setSearchParams(next, { replace: true });
  }, [searchParams, setSearchParams, canWrite]);

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

  // Subtitle counts are derived from the links already loaded on screen (no
  // extra API call): the list has no server-provided grand total, so this
  // reflects the currently loaded/filtered set, not every link in the account.
  const totalClicks = useMemo(() => filtered.reduce((sum, l) => sum + (l.visits ?? 0), 0), [filtered]);
  const tagChips = useMemo(() => tagsQuery.data?.tags ?? [], [tagsQuery.data?.tags]);

  // When collapsed, pin the active tag in the visible set if it's beyond the cap,
  // so a user who expands → selects a tag → collapses still sees the selection.
  const visibleTagChips = useMemo(() => {
    if (showAllTags) {
      return tagChips;
    }
    const sliced = tagChips.slice(0, TAG_CHIPS_VISIBLE_CAP);
    if (tag && !sliced.some((tagItem) => tagItem.name === tag)) {
      const activeTag = tagChips.find((tagItem) => tagItem.name === tag);
      if (activeTag) {
        return [...sliced, activeTag];
      }
    }
    return sliced;
  }, [tagChips, tag, showAllTags]);

  // Count reflects the actual hidden tags; button shows if any tags are beyond the cap.
  const hiddenTagCount = Math.max(0, tagChips.length - visibleTagChips.length);

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
    <div className="flex flex-col gap-4 animate-rise">
      <PageHeader
        title={t("links.heading")}
        subtitle={t("links.countSubtitle", { count: filtered.length, clicks: totalClicks })}
      />

      <div className="flex flex-wrap items-center gap-3">
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
            checked={activeOnly}
            onChange={(e) => setActiveOnly(e.target.checked)}
          />
          {t("links.activeFilterLabel")}
        </label>

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

      {tagChips.length > 0 && (
        <div className="flex flex-wrap items-center gap-2" role="group" aria-label={t("links.tagFilterLabel")}>
          <button type="button" onClick={() => setTag("")} aria-pressed={tag === ""} className={chipClass(tag === "")}>
            {t("links.tagFilterAllOption")}
          </button>
          {visibleTagChips.map((tagOption) => (
            <button
              key={tagOption.name}
              type="button"
              onClick={() => setTag(tagOption.name)}
              aria-pressed={tag === tagOption.name}
              className={chipClass(tag === tagOption.name)}
            >
              {tagOption.name}
              <span className="font-mono text-[11px] opacity-70">{tagOption.count}</span>
            </button>
          ))}
          {tagChips.length > TAG_CHIPS_VISIBLE_CAP && (
            <button
              type="button"
              onClick={() => setShowAllTags((prev) => !prev)}
              aria-expanded={showAllTags}
              className={chipClass(false)}
            >
              {showAllTags ? t("links.lessTags") : t("links.moreTags", { count: hiddenTagCount })}
            </button>
          )}
        </div>
      )}

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
          <CardContent className="flex flex-col items-center gap-3 py-10 text-center">
            <div className="mx-auto flex size-10 items-center justify-center rounded-[9px] bg-accent-wash border border-accent-line">
              <Link2 className="size-[18px] text-brand-ink" aria-hidden="true" />
            </div>
            <div>
              <p className="font-medium">{t("links.emptyTitle")}</p>
              {canWrite && <p className="text-sm text-muted-foreground">{t("links.emptySubtitle")}</p>}
            </div>
            {canWrite && (
              <Button onClick={() => setCreateOpen(true)}>
                <Plus className="size-4" />
                {t("links.createButton")}
              </Button>
            )}
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
        <LinkTable
          links={filtered}
          canWrite={canWrite}
          onEdit={(link) => setEditingLink(link)}
          onDelete={(link) => setDeletingLink(link)}
        />
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
        tags={tagsQuery.data?.tags?.map((tagItem) => tagItem.name) ?? []}
      />

      {editingLink && (
        <EditLinkDialog
          key={editingLink.code}
          link={editingLink}
          open
          onOpenChange={(open) => !open && setEditingLink(null)}
          folders={foldersQuery.data?.folders ?? []}
          tags={tagsQuery.data?.tags?.map((tagItem) => tagItem.name) ?? []}
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
