import { flexRender, getCoreRowModel, useReactTable, type ColumnDef } from "@tanstack/react-table";
import { BarChart3, Check, Copy, MoreHorizontal, Pencil, QrCode, Trash2 } from "lucide-react";
import { useState } from "react";
import { Link as RouterLink, useNavigate } from "react-router-dom";
import { toast } from "sonner";
import { LinkQrDialog } from "@/components/LinkQrDialog";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { formatDate } from "@/lib/format";
import type { Link } from "@/lib/types";

// A base pública dos links curtos é o próprio host da API (é ele quem
// resolve `/:code`); sem essa env var, cai no host onde o painel está
// servido — mais correto do que inventar um domínio. Sem barra final, pra
// não gerar `//` na concatenação com o código.
const PUBLIC_BASE = (
  (import.meta.env.VITE_API_BASE_URL as string | undefined) || window.location.origin
).replace(/\/+$/, "");

function shortUrl(code: string): string {
  return `${PUBLIC_BASE}/${code}`;
}

interface LinkTableProps {
  links: Link[];
  onEdit: (link: Link) => void;
  onDelete: (link: Link) => void;
}

export function LinkTable({ links, onEdit, onDelete }: LinkTableProps) {
  const [justCopiedId, setJustCopiedId] = useState<number | null>(null);
  const [qrLink, setQrLink] = useState<Link | null>(null);
  const navigate = useNavigate();

  async function handleCopy(link: Link) {
    try {
      await navigator.clipboard.writeText(shortUrl(link.code));
      toast.success("Copiado!");
      setJustCopiedId(link.id);
      setTimeout(() => setJustCopiedId((current) => (current === link.id ? null : current)), 1500);
    } catch {
      toast.error("Não foi possível copiar. Copie manualmente.");
    }
  }

  const columns: ColumnDef<Link>[] = [
    {
      accessorKey: "code",
      header: "Código",
      cell: ({ row }) => (
        <RouterLink
          to={`/links/${row.original.code}`}
          className="font-mono text-sm font-medium text-primary hover:underline"
          aria-label={`Ver estatísticas de ${row.original.code}`}
        >
          {row.original.code}
        </RouterLink>
      ),
    },
    {
      accessorKey: "url",
      header: "Destino",
      cell: ({ row }) => (
        <span className="block max-w-64 truncate text-muted-foreground" title={row.original.url}>
          {row.original.url}
        </span>
      ),
    },
    {
      accessorKey: "alias",
      header: "Alias",
      cell: ({ row }) => row.original.alias || <span className="text-muted-foreground">—</span>,
    },
    {
      accessorKey: "created",
      header: "Criado",
      cell: ({ row }) => formatDate(row.original.created),
    },
    {
      accessorKey: "expiry",
      header: "Expira",
      cell: ({ row }) =>
        row.original.expiry == null ? (
          <span className="text-muted-foreground">nunca</span>
        ) : (
          formatDate(row.original.expiry)
        ),
    },
    {
      id: "actions",
      header: () => <span className="sr-only">Ações</span>,
      cell: ({ row }) => {
        const link = row.original;
        const justCopied = justCopiedId === link.id;
        return (
          <div className="flex items-center justify-end gap-1">
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label={`Copiar link curto de ${link.code}`}
              onClick={() => handleCopy(link)}
            >
              {justCopied ? <Check className="size-3.5 text-primary" /> : <Copy className="size-3.5" />}
            </Button>
            <DropdownMenu>
              <DropdownMenuTrigger
                render={
                  <Button variant="ghost" size="icon-sm" aria-label={`Mais ações para ${link.code}`} />
                }
              >
                <MoreHorizontal className="size-3.5" />
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem onClick={() => navigate(`/links/${link.code}`)}>
                  <BarChart3 className="size-3.5" />
                  Estatísticas
                </DropdownMenuItem>
                <DropdownMenuItem onClick={() => setQrLink(link)}>
                  <QrCode className="size-3.5" />
                  QR code
                </DropdownMenuItem>
                <DropdownMenuItem onClick={() => onEdit(link)}>
                  <Pencil className="size-3.5" />
                  Editar
                </DropdownMenuItem>
                <DropdownMenuItem variant="destructive" onClick={() => onDelete(link)}>
                  <Trash2 className="size-3.5" />
                  Excluir
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
        <caption className="sr-only">Links curtos cadastrados no sistema</caption>
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
