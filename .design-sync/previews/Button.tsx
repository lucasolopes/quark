// Button preview — variant axis, sizes, and static states.
import { Plus, Trash2, Copy } from "lucide-react";
import { Button } from "web";

export function Variants() {
  return (
    <div className="flex flex-wrap items-center gap-3">
      <Button>Create link</Button>
      <Button variant="secondary">Duplicate</Button>
      <Button variant="outline">Export CSV</Button>
      <Button variant="ghost">Cancel</Button>
      <Button variant="destructive">Delete link</Button>
      <Button variant="link">View analytics</Button>
    </div>
  );
}

export function Sizes() {
  return (
    <div className="flex flex-wrap items-center gap-3">
      <Button size="lg">Create link</Button>
      <Button size="default">Create link</Button>
      <Button size="sm">Create link</Button>
      <Button size="xs">Create link</Button>
    </div>
  );
}

export function WithIcons() {
  return (
    <div className="flex flex-wrap items-center gap-3">
      <Button>
        <Plus data-icon="inline-start" /> New link
      </Button>
      <Button variant="outline" size="icon" aria-label="Copy short URL">
        <Copy />
      </Button>
      <Button variant="destructive" size="icon-sm" aria-label="Delete">
        <Trash2 />
      </Button>
    </div>
  );
}

export function Disabled() {
  return (
    <div className="flex flex-wrap items-center gap-3">
      <Button disabled>Create link</Button>
      <Button variant="outline" disabled>
        Export CSV
      </Button>
    </div>
  );
}
