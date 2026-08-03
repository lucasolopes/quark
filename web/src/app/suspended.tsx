import { Loader2 } from "lucide-react";
import { Suspense, type ReactElement } from "react";

/** Fallback shown while a lazy route chunk loads. */
function RouteFallback() {
  return (
    <div className="flex min-h-[60vh] items-center justify-center" aria-hidden="true">
      <Loader2 className="size-6 animate-spin text-muted-foreground" />
    </div>
  );
}

/**
 * Wrap a lazy route element in Suspense so its chunk can load without blocking.
 *
 * Mora num modulo proprio, e nao no `router.tsx`, porque o barrel Enterprise
 * (`web/src/ee/index.tsx`) tambem monta rotas e precisa do mesmo wrapper sem
 * criar import circular (LUC-19).
 */
export function suspended(element: ReactElement): ReactElement {
  return <Suspense fallback={<RouteFallback />}>{element}</Suspense>;
}
