import { flexRender, getCoreRowModel, useReactTable, type ColumnDef } from "@tanstack/react-table";
import { BarChart3, Check, Copy, MoreHorizontal, Pencil, QrCode, Trash2 } from "lucide-react";
import { useState } from "react";
import { Link as RouterLink, useNavigate } from "react-router-dom";
import { toast } from "sonner";
import { LinkQrDialog } from "@/components/LinkQrDialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { useT } from "@/i18n";
import { formatDate } from "@/lib/format";
import type { Link } from "@/lib/types";

/**
 * The public base for short links is the API host itself (it resolves `/:code`);
 * without this env var, falls back to the host serving the panel — more correct
 * than inventing a domain. No trailing slash, to avoid `//` when concatenated
 * with the code.
 */
const PUBLIC_BASE = (
  (import.meta.env.VITE_API_BASE_URL as string | undefined) || window.location.origin
).replace(/\/+$/, "");

function shortUrl(code: string): string {
  return `${PUBLIC_BASE}/${code}`;
}

/** Max tag badges shown per row before collapsing the rest into a "+k" badge. */
const MAX_VISIBLE_TAGS = 3;

interface LinkTableProps {
  links: Link[];
  onEdit: (link: Link) => void;
  onDelete: (link: Link) => void;
}

export function LinkTable({ links, onEdit, onDelete }: LinkTableProps) {
  const t = useT();
  const [justCopiedId, setJustCopiedId] = useState<number | null>(null);
  const [qrLink, setQrLink] = useState<Link | null>(null);
  const navigate = useNavigate();

  async function handleCopy(link: Link) {
    try {
      await navigator.clipboard.writeText(shortUrl(link.code));
      toast.success(t("linkTable.copied"));
      setJustCopiedId(link.id);
      setTimeout(() => setJustCopiedId((current) => (current === link.id ? null : current)), 1500);
    } catch {
      toast.error(t("linkTable.copyFailed"));
    }
  }

  const columns: ColumnDef<Link>[] = [
    {
      accessorKey: "code",
      header: t("linkTable.columnCode"),
      cell: ({ row }) => (
        <RouterLink
          to={`/links/${row.original.code}`}
          className="font-mono text-sm font-medium text-primary hover:underline"
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
        <span className="block max-w-64 truncate text-muted-foreground" title={row.original.url}>
          {row.original.url}
        </span>
      ),
    },
    {
      accessorKey: "alias",
      header: t("linkTable.columnAlias"),
      cell: ({ row }) => row.original.alias || <span className="text-muted-foreground">—</span>,
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
            {visible.map((tag) => (
              <Badge key={tag} variant="secondary">
                {tag}
              </Badge>
            ))}
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
              {justCopied ? <Check className="size-3.5 text-primary" /> : <Copy className="size-3.5" />}
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
                <DropdownMenuItem onClick={() => onEdit(link)}>
                  <Pencil className="size-3.5" />
                  {t("linkTable.editMenuItem")}
                </DropdownMenuItem>
                <DropdownMenuItem variant="destructive" onClick={() => onDelete(link)}>
                  <Trash2 className="size-3.5" />
                  {t("linkTable.deleteMenuItem")}
                </DropdownMenuItem>
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
        <LinkQrDialog
          code={qrLink.code}
          url={shortUrl(qrLink.code)}
          open
          onOpenChange={(next) => {
            if (!next) setQrLink(null);
          }}
        />
      )}
    </>
  );
}
