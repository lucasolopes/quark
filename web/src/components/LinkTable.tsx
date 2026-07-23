import { flexRender, getCoreRowModel, useReactTable, type ColumnDef } from "@tanstack/react-table";
import { BarChart3, Check, Copy, Folder, Lock, MoreHorizontal, Pencil, QrCode, Trash2, X } from "lucide-react";
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
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { useT } from "@/i18n";
import { formatDate } from "@/lib/format";
import { useBulkLinks, useMe } from "@/lib/queries";
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

/** Hosts the cloud may provide for building a short URL. */
interface TenantDomain {
  /** The server-resolved primary host (LUC-86): primary custom domain →
   * subdomain → shared host. When present it wins over everything below. */
  primaryHost?: string | null;
  slug?: string | null;
  suffix?: string | null;
  /** Shared short-link host (`QUARK_PUBLIC_HOST`), fallback before the API origin. */
  publicHost?: string | null;
}

/**
 * Builds the short URL shown/copied for a code. Prefers the tenant's
 * server-resolved primary host (which already prioritizes a verified custom
 * domain over the subdomain). Falls back to `<slug>.<suffix>`, then the shared
 * host, then `PUBLIC_BASE` (the API origin, OSS).
 */
function buildShortUrl(code: string, { primaryHost, slug, suffix, publicHost }: TenantDomain): string {
  if (primaryHost) return `https://${primaryHost}/${code}`;
  if (slug && suffix) return `https://${slug}.${suffix}/${code}`;
  if (publicHost) return `https://${publicHost}/${code}`;
  return `${PUBLIC_BASE}/${code}`;
}

/** Max tag badges shown per row before collapsing the rest into a "+k" badge. */
const MAX_VISIBLE_TAGS = 3;

interface LinkTableProps {
  links: Link[];
  onEdit: (link: Link) => void;
  onDelete: (link: Link) => void;
  /** When false (a Viewer), write affordances are hidden: bulk selection, and
   * the Edit/Delete row actions. Defaults to true so read-only callers and
   * existing tests keep the full table. The backend enforces this regardless. */
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
  const tenantDomain: TenantDomain = { primaryHost: me?.primary_link_host, slug: currentMembership?.slug, suffix: me?.tenant_domain_suffix, publicHost: me?.public_host };

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

  const columns: ColumnDef<Link>[] = [
    // Bulk selection is a write affordance (bulk edit/delete), so a Viewer
    // never gets the select column.
    ...(canWrite
      ? [
          {
            id: "select",
            header: () => (
              <Checkbox
                checked={allSelected}
                indeterminate={someSelected}
                onCheckedChange={(checked) => toggleAll(checked === true)}
                aria-label={t("linkTable.selectAllAria")}
              />
            ),
            cell: ({ row }) => (
              <Checkbox
                checked={selected.has(row.original.code)}
                onCheckedChange={(checked) => toggleRow(row.original.code, checked === true)}
                aria-label={t("linkTable.selectRowAria", { code: row.original.code })}
              />
            ),
          } satisfies ColumnDef<Link>,
        ]
      : []),
    {
      accessorKey: "code",
      header: t("linkTable.columnCode"),
      cell: ({ row }) => (
        <RouterLink
          to={`/links/${row.original.code}`}
          className="font-mono text-sm font-medium text-brand-ink hover:underline"
          aria-label={t("linkTable.viewStatsAria", { code: row.original.code })}
        >
          {row.original.code}
        </RouterLink>
      ),
    },
    {
      accessorKey: "url",
      header: t("linkTable.columnDestination"),
      cell: ({ row }) => (
        <div className="flex max-w-64 items-center gap-1.5">
          <span className="truncate text-muted-foreground" title={row.original.url}>
            {row.original.url}
          </span>
          {row.original.rules.length > 0 && (
            <Badge variant="secondary" className="shrink-0">
              {t("linkTable.rulesBadge", { count: row.original.rules.length })}
            </Badge>
          )}
          {row.original.variants.length > 0 && (
            <Badge variant="secondary" className="shrink-0">
              {t("linkTable.variantsBadge", { count: row.original.variants.length })}
            </Badge>
          )}
          {row.original.has_password && (
            <Lock
              className="size-3.5 shrink-0 text-muted-foreground"
              aria-label={t("linkTable.protectedAria")}
            />
          )}
          {row.original.health && (
            <span
              role="img"
              aria-label={
                row.original.health.healthy
                  ? t("linkTable.healthOk")
                  : t("linkTable.healthBroken", { status: row.original.health.status ?? "—" })
              }
              title={
                row.original.health.healthy
                  ? t("linkTable.healthOk")
                  : t("linkTable.healthBroken", { status: row.original.health.status ?? "—" })
              }
              className={`size-2 shrink-0 rounded-full ${
                row.original.health.healthy ? "bg-emerald-500" : "bg-red-500"
              }`}
            />
          )}
        </div>
      ),
    },
    {
      accessorKey: "alias",
      header: t("linkTable.columnAlias"),
      cell: ({ row }) => row.original.alias || <span className="text-muted-foreground">—</span>,
    },
    {
      id: "folder",
      header: t("linkTable.columnFolder"),
      cell: ({ row }) => {
        const folder = row.original.folder;
        if (!folder) return <span className="text-muted-foreground">—</span>;
        return (
          <Badge variant="outline" className="gap-1 font-normal">
            <Folder className="size-3" aria-hidden="true" />
            {folder}
          </Badge>
        );
      },
    },
    {
      id: "tags",
      header: t("linkTable.columnTags"),
      cell: ({ row }) => {
        const tags = row.original.tags ?? [];
        if (tags.length === 0) return <span className="text-muted-foreground">—</span>;
        const visible = tags.slice(0, MAX_VISIBLE_TAGS);
        const hiddenCount = tags.length - visible.length;
        return (
          <div className="flex flex-wrap gap-1">
            {visible.map((tag) => {
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
            {hiddenCount > 0 && <Badge variant="outline">{t("linkTable.moreTags", { count: hiddenCount })}</Badge>}
          </div>
        );
      },
    },
    {
      accessorKey: "created",
      header: t("linkTable.columnCreated"),
      cell: ({ row }) => formatDate(row.original.created),
    },
    {
      accessorKey: "expiry",
      header: t("linkTable.columnExpires"),
      cell: ({ row }) =>
        row.original.expiry == null ? (
          <span className="text-muted-foreground">{t("linkTable.never")}</span>
        ) : (
          formatDate(row.original.expiry)
        ),
    },
    {
      accessorKey: "visits",
      header: t("linkTable.columnVisits"),
      cell: ({ row }) =>
        row.original.max_visits ? (
          <span>{`${row.original.visits} / ${row.original.max_visits}`}</span>
        ) : (
          <span className="text-muted-foreground">{row.original.visits}</span>
        ),
    },
    {
      id: "actions",
      header: () => <span className="sr-only">{t("linkTable.actionsSr")}</span>,
      cell: ({ row }) => {
        const link = row.original;
        const justCopied = justCopiedId === link.id;
        return (
          <div className="flex items-center justify-end gap-1">
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label={t("linkTable.copyAria", { code: link.code })}
              onClick={() => handleCopy(link)}
            >
              {justCopied ? <Check className="size-3.5 text-brand-ink" /> : <Copy className="size-3.5" />}
            </Button>
            <DropdownMenu>
              <DropdownMenuTrigger
                render={
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    aria-label={t("linkTable.moreActionsAria", { code: link.code })}
                  />
                }
              >
                <MoreHorizontal className="size-3.5" />
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem onClick={() => navigate(`/links/${link.code}`)}>
                  <BarChart3 className="size-3.5" />
                  {t("linkTable.statsMenuItem")}
                </DropdownMenuItem>
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
        );
      },
    },
  ];

  const table = useReactTable({ data: links, columns, getCoreRowModel: getCoreRowModel() });

  return (
    <>
      {selected.size > 0 && (
        <div className="flex flex-wrap items-center gap-2 border-b px-4 py-3">
          <span className="text-sm font-medium">
            {t("linkTable.selected", { count: selected.size })}
          </span>
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
          <Button
            variant="outline"
            size="sm"
            disabled={bulkLinks.isPending}
            onClick={() => runTagOp("add_tag")}
          >
            {t("linkTable.bulkAddTag")}
          </Button>
          <Button
            variant="outline"
            size="sm"
            disabled={bulkLinks.isPending}
            onClick={() => runTagOp("remove_tag")}
          >
            {t("linkTable.bulkRemoveTag")}
          </Button>
          <Button
            variant="outline"
            size="sm"
            disabled={bulkLinks.isPending}
            onClick={runSetFolder}
          >
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
        </div>
      )}

      <Table>
        <caption className="sr-only">{t("linkTable.caption")}</caption>
        <TableHeader>
          {table.getHeaderGroups().map((headerGroup) => (
            <TableRow key={headerGroup.id}>
              {headerGroup.headers.map((header) => (
                <TableHead key={header.id}>
                  {flexRender(header.column.columnDef.header, header.getContext())}
                </TableHead>
              ))}
            </TableRow>
          ))}
        </TableHeader>
        <TableBody>
          {table.getRowModel().rows.map((row) => (
            <TableRow key={row.id}>
              {row.getVisibleCells().map((cell) => (
                <TableCell key={cell.id}>{flexRender(cell.column.columnDef.cell, cell.getContext())}</TableCell>
              ))}
            </TableRow>
          ))}
        </TableBody>
      </Table>

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
