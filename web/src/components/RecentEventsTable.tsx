import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { useT } from "@/i18n";
import { formatDateTime } from "@/lib/format";
import type { ClickEvent } from "@/lib/types";

interface RecentEventsTableProps {
  events: ClickEvent[];
}

/**
 * Table of recent clicks, newest first.
 *
 * `ClickEvent.id` is the LINK's id (repeated on every event — the backend
 * does not assign a per-click id), so it CANNOT be used as the React `key`:
 * every row would collide and React would confuse updates between them. The
 * array index (stable for a list that is only ever replaced, never reordered
 * in memory) is the correct key here.
 */
export function RecentEventsTable({ events }: RecentEventsTableProps) {
  const t = useT();
  const sorted = [...events].sort((a, b) => b.ts - a.ts);

  if (sorted.length === 0) {
    return <p className="py-8 text-center text-sm text-muted-foreground">{t("events.empty")}</p>;
  }

  return (
    <Table>
      <caption className="sr-only">{t("events.caption")}</caption>
      <TableHeader>
        <TableRow>
          <TableHead>{t("events.timeHeader")}</TableHead>
          <TableHead>{t("events.countryHeader")}</TableHead>
          <TableHead>{t("events.cityHeader")}</TableHead>
          <TableHead>{t("events.refererHeader")}</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {sorted.map((event, i) => (
          <TableRow key={`${event.ts}-${i}`}>
            <TableCell>{formatDateTime(event.ts)}</TableCell>
            <TableCell>{event.country || <span className="text-muted-foreground">—</span>}</TableCell>
            <TableCell>{event.city || <span className="text-muted-foreground">—</span>}</TableCell>
            <TableCell className="max-w-64 truncate" title={event.referer ?? undefined}>
              {event.referer || <span className="text-muted-foreground">{t("events.direct")}</span>}
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
