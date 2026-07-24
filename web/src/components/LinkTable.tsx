import { getCoreRowModel, useReactTable, type ColumnDef } from "@tanstack/react-table";
import { BarChart3, Check, Copy, Folder, Link2, Lock, MoreHorizontal, Pencil, QrCode, Trash2, X } from "lucide-react";
import { lazy, Suspense, useEffect, useState } from "react";
import { Link as RouterLink, useNavigate } from "react-router-dom";
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
import { Checkbox } from "@/components/ui/checkbox";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { useT } from "@/i18n";
import { useBulkLinks, useMe } from "@/lib/queries";
import { resolveShortHost, type TenantDomainHost } from "@/lib/short-url";
import { tagColor } from "@/lib/tag-color";
import type { BulkOp, Link } from "@/lib/types";

// qrcode.react is only needed when the QR dialog is opened; lazy-load it so it
// lands in its own chunk instead of the main bundle.
const LinkQrDialog = lazy(() => import("@/components/LinkQrDialog").then((m) => ({ default: m.LinkQrDialog })));

/**
 * The public base for short links is the API host itself (it resolves `/:code`);
 * without this env var, falls back to the host serving the panel — more correct
 * than inventing a domain. No trailing slash, to avoid `//` when concatenated
 * with the code.
 */
const PUBLIC_BASE = (
  (import.meta.env.VITE_API_BASE_URL as string | undefined) || window.location.origin
).replace(/\/+$/, "");

/**
 * Builds the short URL shown/copied for a code. The host itself — tenant's
 * server-resolved primary host, else `<slug>.<suffix>`, else the shared
 * public host, else the panel's own origin — comes from `resolveShortHost`
 * (`@/lib/short-url`), the single source for that precedence; this only adds
 * the protocol on top: an explicit tenant host is always `https://`, while
 * the origin fallback keeps whatever protocol `PUBLIC_BASE` already has (dev
 * often serves the panel over `http`, and forcing `https://` there would
 * produce a link that doesn't match how the app itself is being served).
 */
function buildShortUrl(code: string, domain: TenantDomainHost): string {
  const { primaryHost, slug, suffix, publicHost } = domain;
  const host = resolveShortHost(domain);
  const hasTenantHost = Boolean(primaryHost || (slug && suffix) || publicHost);
  return hasTenantHost ? `https://${host}/${code}` : `${PUBLIC_BASE}/${code}`;
}

/** Max tag badges shown per card before collapsing the rest into a "+k" badge. */
const MAX_VISIBLE_TAGS = 3;

interface LinkTableProps {
  links: Link[];
  onEdit: (link: Link) => void;
  onDelete: (link: Link) => void;
  /** When false (a Viewer), write affordances are hidden: bulk selection, and
   * the Edit/Delete row actions. Defaults to true so read-only callers and
   * existing tests keep the full card. The backend enforces this regardless. */
  canWrite?: boolean;
}

export function LinkTable({ links, onEdit, onDelete, canWrite = true }: LinkTableProps) {
  const t = useT();
  const [justCopiedId, setJustCopiedId] = useState<number | null>(null);
  const [qrLink, setQrLink] = useState<Link | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [bulkValue, setBulkValue] = useState("");
  const [confirmingBulkDelete, setConfirmingBulkDelete] = useState(false);
  const navigate = useNavigate();
  const { data: me } = useMe();
  const bulkLinks = useBulkLinks();
  const currentMembership = me?.memberships?.find((m) => m.tenant_id === me.current_tenant);
  const tenantDomain: TenantDomainHost = { primaryHost: me?.primary_link_host, slug: currentMembership?.slug, suffix: me?.tenant_domain_suffix, publicHost: me?.public_host };

  const pageCodes = links.map((l) => l.code);
  const allSelected = pageCodes.length > 0 && pageCodes.every((c) => selected.has(c));
  const someSelected = pageCodes.some((c) => selected.has(c)) && !allSelected;

  // Prune the selection to codes still present after the list refetches (a
  // bulk delete or a filter change can drop rows out from under a stale set).
  useEffect(() => {
    setSelected((prev) => {
      const next = new Set([...prev].filter((c) => pageCodes.includes(c)));
      return next.size === prev.size ? prev : next;
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [links]);

  function toggleRow(code: string, checked: boolean) {
    setSelected((prev) => {
      const next = new Set(prev);
      if (checked) next.add(code);
      else next.delete(code);
      return next;
    });
  }

  function toggleAll(checked: boolean) {
    setSelected(checked ? new Set(pageCodes) : new Set());
  }

  function clearSelection() {
    setSelected(new Set());
  }

  async function runBulk(op: BulkOp, value?: string) {
    const codes = [...selected];
    if (codes.length === 0) return;
    try {
      const report = await bulkLinks.mutateAsync({ codes, op, value });
      if (report.failed > 0) {
        toast.warning(t("linkTable.bulkPartial", { ok: report.ok, failed: report.failed }));
      } else {
        toast.success(t("linkTable.bulkDone", { ok: report.ok }));
      }
      setBulkValue("");
      clearSelection();
    } catch {
      toast.error(t("linkTable.bulkError"));
    }
  }

  function runTagOp(op: "add_tag" | "remove_tag") {
    if (bulkValue.trim() === "") {
      toast.error(t("linkTable.bulkNeedsValue"));
      return;
    }
    void runBulk(op, bulkValue.trim());
  }

  // Guard the empty value: a blank folder would clear the folder on every
  // selected link at once, silently. Bulk mass-clear is not an intended
  // action from this button, so require a non-empty folder name.
  function runSetFolder() {
    if (bulkValue.trim() === "") {
      toast.error(t("linkTable.bulkNeedsValue"));
      return;
    }
    void runBulk("set_folder", bulkValue.trim());
  }

  async function handleCopy(link: Link) {
    try {
      await navigator.clipboard.writeText(buildShortUrl(link.code, tenantDomain));
      toast.success(t("linkTable.copied"));
      setJustCopiedId(link.id);
      setTimeout(() => setJustCopiedId((current) => (current === link.id ? null : current)), 1500);
    } catch {
      toast.error(t("linkTable.copyFailed"));
    }
  }

  // TanStack Table still owns the row model (and can drive headless sorting or
  // filtering later); the screen just renders each row as a card instead of a
  // table row. These accessor columns keep the model addressable by field.
  const columns: Array<ColumnDef<Link>> = [
    { accessorKey: "code" },
    { accessorKey: "url" },
    { accessorKey: "alias" },
    { id: "folder", accessorFn: (l) => l.folder ?? "" },
    { accessorKey: "visits" },
    { accessorKey: "created" },
    { accessorKey: "expiry" },
  ];

  const table = useReactTable({ data: links, columns, getCoreRowModel: getCoreRowModel() });

  return (
    <>
      {canWrite && (
        <div className="mb-3 flex flex-wrap items-center gap-2">
          <Checkbox
            checked={allSelected}
            indeterminate={someSelected}
            onCheckedChange={(checked) => toggleAll(checked === true)}
            aria-label={t("linkTable.selectAllAria")}
          />
          {selected.size > 0 ? (
            <>
              <span className="text-sm font-medium">{t("linkTable.selected", { count: selected.size })}</span>
              <Button variant="ghost" size="sm" onClick={clearSelection}>
                <X className="size-3.5" />
                {t("linkTable.clearSelection")}
              </Button>
              <div className="mx-1 h-5 w-px bg-border" aria-hidden="true" />
              <Input
                value={bulkValue}
                onChange={(e) => setBulkValue(e.target.value)}
                placeholder={t("linkTable.bulkValuePlaceholder")}
                aria-label={t("linkTable.bulkValuePlaceholder")}
                className="h-8 w-48"
                disabled={bulkLinks.isPending}
              />
              <Button variant="outline" size="sm" disabled={bulkLinks.isPending} onClick={() => runTagOp("add_tag")}>
                {t("linkTable.bulkAddTag")}
              </Button>
              <Button variant="outline" size="sm" disabled={bulkLinks.isPending} onClick={() => runTagOp("remove_tag")}>
                {t("linkTable.bulkRemoveTag")}
              </Button>
              <Button variant="outline" size="sm" disabled={bulkLinks.isPending} onClick={runSetFolder}>
                {t("linkTable.bulkSetFolder")}
              </Button>
              <Button
                variant="destructive"
                size="sm"
                disabled={bulkLinks.isPending}
                onClick={() => setConfirmingBulkDelete(true)}
              >
                <Trash2 className="size-3.5" />
                {t("linkTable.bulkDelete")}
              </Button>
            </>
          ) : (
            <span className="text-sm text-muted-foreground">{t("linkTable.selectAllAria")}</span>
          )}
        </div>
      )}

      <ul className="flex flex-col gap-2.5">
        {table.getRowModel().rows.map((row) => {
          const link = row.original;
          const justCopied = justCopiedId === link.id;
          const tags = link.tags ?? [];
          const visibleTags = tags.slice(0, MAX_VISIBLE_TAGS);
          const hiddenTags = tags.length - visibleTags.length;
          const clicks = link.visits ?? 0;
          const healthLabel = link.health
            ? link.health.healthy
              ? t("linkTable.healthOk")
              : t("linkTable.healthBroken", { status: link.health.status ?? "—" })
            : "";
          return (
            <li
              key={link.code}
              data-testid="link-card"
              className="card-hover flex items-center gap-4 rounded-lg border border-border bg-card p-4 shadow-card"
            >
              {canWrite && (
                <Checkbox
                  checked={selected.has(link.code)}
                  onCheckedChange={(checked) => toggleRow(link.code, checked === true)}
                  aria-label={t("linkTable.selectRowAria", { code: link.code })}
                />
              )}

              <div className="flex size-10 shrink-0 items-center justify-center rounded-[9px] border border-accent-line bg-accent-wash">
                <Link2 className="size-[18px] text-brand-ink" aria-hidden="true" />
              </div>

              <div className="min-w-0 flex-1">
                <div className="flex flex-wrap items-center gap-2">
                  <RouterLink
                    to={`/links/${link.code}`}
                    className="font-mono text-[14.5px] font-medium text-brand-ink hover:underline"
                    aria-label={t("linkTable.viewStatsAria", { code: link.code })}
                  >
                    {link.code}
                  </RouterLink>
                  {link.alias && <Badge variant="secondary">{link.alias}</Badge>}
                  {link.folder && (
                    <Badge variant="outline" className="gap-1 font-normal">
                      <Folder className="size-3" aria-hidden="true" />
                      {link.folder}
                    </Badge>
                  )}
                </div>

                <div className="mt-1 flex items-center gap-1.5">
                  <span className="max-w-[440px] truncate text-[13px] text-muted-foreground" title={link.url}>
                    {link.url}
                  </span>
                  {link.rules.length > 0 && (
                    <Badge variant="secondary" className="shrink-0">
                      {t("linkTable.rulesBadge", { count: link.rules.length })}
                    </Badge>
                  )}
                  {link.variants.length > 0 && (
                    <Badge variant="secondary" className="shrink-0">
                      {t("linkTable.variantsBadge", { count: link.variants.length })}
                    </Badge>
                  )}
                  {link.has_password && (
                    <Lock
                      className="size-3.5 shrink-0 text-muted-foreground"
                      aria-label={t("linkTable.protectedAria")}
                    />
                  )}
                  {link.health && (
                    <span
                      role="img"
                      aria-label={healthLabel}
                      title={healthLabel}
                      className={`size-2 shrink-0 rounded-full ${
                        link.health.healthy ? "bg-primary" : "bg-destructive"
                      }`}
                    />
                  )}
                </div>

                {tags.length > 0 && (
                  <div className="mt-1.5 flex flex-wrap gap-1.5">
                    {visibleTags.map((tag) => {
                      const color = tagColor(tag);
                      return (
                        <Badge
                          key={tag}
                          variant="secondary"
                          className="gap-1.5 border-transparent"
                          style={{ backgroundColor: color.bg, color: color.text }}
                        >
                          <span
                            aria-hidden="true"
                            className="size-1.5 shrink-0 rounded-full"
                            style={{ backgroundColor: color.dot }}
                          />
                          {tag}
                        </Badge>
                      );
                    })}
                    {hiddenTags > 0 && <Badge variant="outline">{t("linkTable.moreTags", { count: hiddenTags })}</Badge>}
                  </div>
                )}
              </div>

              <div className="shrink-0 text-right">
                <div className="font-heading text-lg font-bold tabular-nums text-strong">
                  {link.max_visits ? `${clicks} / ${link.max_visits}` : clicks}
                </div>
                <div className="text-[11px] text-muted-foreground">{t("links.clicks")}</div>
              </div>

              <div className="flex shrink-0 items-center gap-1">
                <Button
                  variant="outline"
                  size="icon"
                  aria-label={t("linkTable.copyAria", { code: link.code })}
                  onClick={() => handleCopy(link)}
                >
                  {justCopied ? <Check className="size-3.5 text-brand-ink" /> : <Copy className="size-3.5" />}
                </Button>
                <Button
                  variant="outline"
                  size="icon"
                  aria-label={t("linkTable.viewStatsAria", { code: link.code })}
                  onClick={() => navigate(`/links/${link.code}`)}
                >
                  <BarChart3 className="size-3.5" />
                </Button>
                <DropdownMenu>
                  <DropdownMenuTrigger
                    render={
                      <Button
                        variant="outline"
                        size="icon"
                        aria-label={t("linkTable.moreActionsAria", { code: link.code })}
                      />
                    }
                  >
                    <MoreHorizontal className="size-3.5" />
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="end">
                    <DropdownMenuItem onClick={() => setQrLink(link)}>
                      <QrCode className="size-3.5" />
                      {t("linkTable.qrMenuItem")}
                    </DropdownMenuItem>
                    {canWrite && (
                      <>
                        <DropdownMenuItem onClick={() => onEdit(link)}>
                          <Pencil className="size-3.5" />
                          {t("linkTable.editMenuItem")}
                        </DropdownMenuItem>
                        <DropdownMenuItem variant="destructive" onClick={() => onDelete(link)}>
                          <Trash2 className="size-3.5" />
                          {t("linkTable.deleteMenuItem")}
                        </DropdownMenuItem>
                      </>
                    )}
                  </DropdownMenuContent>
                </DropdownMenu>
              </div>
            </li>
          );
        })}
      </ul>

      {qrLink && (
        <Suspense fallback={null}>
          <LinkQrDialog
            code={qrLink.code}
            url={buildShortUrl(qrLink.code, tenantDomain)}
            open
            onOpenChange={(next) => {
              if (!next) setQrLink(null);
            }}
          />
        </Suspense>
      )}

      <AlertDialog
        open={confirmingBulkDelete}
        onOpenChange={(open) => !open && setConfirmingBulkDelete(false)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("linkTable.bulkDeleteTitle", { count: selected.size })}</AlertDialogTitle>
            <AlertDialogDescription>{t("linkTable.bulkDeleteDescription")}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={bulkLinks.isPending}>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              disabled={bulkLinks.isPending}
              onClick={() => {
                setConfirmingBulkDelete(false);
                void runBulk("delete");
              }}
            >
              {t("linkTable.bulkConfirmDelete")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
