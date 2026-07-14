import { cn } from "@/lib/utils";

/**
 * quark's official mark — the "Feistel-crossing" glyph: four lime nodes
 * (the L/R halves entering and leaving), the X (the per-round swap) and the
 * central ring (the ARX round function). Encodes the engine itself
 * (keyed reversible permutation). Drawn in `currentColor` by default to
 * inherit the context's color; pass `className="text-primary"` for lime.
 */
export function QuarkMark({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 48 48"
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
      role="img"
      aria-label="quark"
      className={cn("size-6", className)}
    >
      <g stroke="currentColor" strokeWidth="4" strokeLinecap="round" fill="none">
        <path d="M14 14 L34 34" />
        <path d="M34 14 L14 34" />
      </g>
      <circle cx="24" cy="24" r="7" fill="none" stroke="currentColor" strokeWidth="4" />
      <g fill="currentColor">
        <circle cx="14" cy="14" r="2.6" />
        <circle cx="34" cy="14" r="2.6" />
        <circle cx="14" cy="34" r="2.6" />
        <circle cx="34" cy="34" r="2.6" />
      </g>
    </svg>
  );
}
