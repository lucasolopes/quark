// QuarkMark preview — the Feistel-crossing glyph in brand color and sizes.
import { QuarkMark } from "web";

export function BrandLime() {
  return (
    <div className="flex items-center gap-6">
      <QuarkMark className="size-12 text-primary glow-glyph" />
      <div>
        <div className="font-heading text-xl font-bold tracking-display text-strong">quark</div>
        <div className="text-subtitle text-muted-foreground">keyed reversible permutation</div>
      </div>
    </div>
  );
}

export function Sizes() {
  return (
    <div className="flex items-end gap-5 text-foreground">
      <QuarkMark className="size-12 text-primary" />
      <QuarkMark className="size-8" />
      <QuarkMark className="size-6 text-muted-foreground" />
    </div>
  );
}
