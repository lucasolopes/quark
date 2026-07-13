import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { formatDateTime } from "@/lib/format";
import type { ClickEvent } from "@/lib/types";

interface RecentEventsTableProps {
  events: ClickEvent[];
}

/**
 * Tabela dos cliques recentes, mais novo primeiro.
 *
 * `ClickEvent.id` é o id do LINK (repetido em todo evento — o backend não
 * atribui id por clique), então NÃO pode ser usado como `key` do React: todas
 * as linhas colidiriam e o React confundiria as atualizações entre elas. O
 * índice do array (estável para uma lista que só é substituída, nunca
 * reordenada em memória) é a chave correta aqui.
 */
export function RecentEventsTable({ events }: RecentEventsTableProps) {
  const sorted = [...events].sort((a, b) => b.ts - a.ts);

  if (sorted.length === 0) {
    return <p className="py-8 text-center text-sm text-muted-foreground">Nenhum clique recente.</p>;
  }

  return (
    <Table>
      <caption className="sr-only">Cliques recentes deste link, do mais novo para o mais antigo</caption>
      <TableHeader>
        <TableRow>
          <TableHead>Horário</TableHead>
          <TableHead>País</TableHead>
          <TableHead>Referência</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {sorted.map((event, i) => (
          <TableRow key={`${event.ts}-${i}`}>
            <TableCell>{formatDateTime(event.ts)}</TableCell>
            <TableCell>{event.country || <span className="text-muted-foreground">—</span>}</TableCell>
            <TableCell className="max-w-64 truncate" title={event.referer ?? undefined}>
              {event.referer || <span className="text-muted-foreground">direto</span>}
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
