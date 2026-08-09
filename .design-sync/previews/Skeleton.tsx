// Skeleton preview — loading shapes as used on the stats page.
import { Skeleton } from "web";

export function LoadingCard() {
  return (
    <div className="w-80 rounded-lg border border-border bg-card p-4">
      <Skeleton className="h-3.5 w-28" />
      <Skeleton className="mt-3 h-8 w-24" />
      <div className="mt-4 flex gap-2">
        <Skeleton className="h-5 w-16" />
        <Skeleton className="h-5 w-12" />
      </div>
    </div>
  );
}

export function TableRows() {
  return (
    <div className="flex w-96 flex-col gap-2.5">
      <Skeleton className="h-4 w-full" />
      <Skeleton className="h-4 w-[85%]" />
      <Skeleton className="h-4 w-[70%]" />
    </div>
  );
}
