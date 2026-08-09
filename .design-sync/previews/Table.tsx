// Table preview — links listing shape from the panel.
import {
  Badge,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "web";

const rows = [
  { code: "spring-sale", dest: "example.com/pricing", clicks: "12,408", tag: "marketing" },
  { code: "gh-readme", dest: "github.com/acme/quark", clicks: "8,102", tag: "docs" },
  { code: "q3-launch", dest: "example.com/launch", clicks: "3,551", tag: "social" },
];

export function LinksTable() {
  return (
    <Table className="w-[620px]">
      <TableHeader>
        <TableRow>
          <TableHead>Code</TableHead>
          <TableHead>Destination</TableHead>
          <TableHead className="text-right">Clicks</TableHead>
          <TableHead>Tag</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map((r) => (
          <TableRow key={r.code}>
            <TableCell className="font-mono text-brand-ink">/{r.code}</TableCell>
            <TableCell className="text-muted-foreground">{r.dest}</TableCell>
            <TableCell className="text-right font-mono">{r.clicks}</TableCell>
            <TableCell>
              <Badge>{r.tag}</Badge>
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
