// Badge preview — variant sweep (mono default/secondary are the panel's tag chips).
import { Badge } from "web";

export function Variants() {
  return (
    <div className="flex flex-wrap items-center gap-3">
      <Badge>marketing</Badge>
      <Badge variant="secondary">q3-launch</Badge>
      <Badge variant="destructive">expired</Badge>
      <Badge variant="outline">Pro</Badge>
      <Badge variant="ghost">draft</Badge>
      <Badge variant="link">docs</Badge>
    </div>
  );
}

export function TagRow() {
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      <Badge>utm-summer</Badge>
      <Badge>social</Badge>
      <Badge variant="secondary">+3</Badge>
    </div>
  );
}
