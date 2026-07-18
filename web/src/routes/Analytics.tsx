import { AlertTriangle, Link2, Loader2 } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { StatsView } from "@/components/StatsView";
import { useDebounce } from "@/hooks/useDebounce";
import { useT } from "@/i18n";
import { ApiError } from "@/lib/api";
import { useLinks } from "@/lib/queries";
import type { Link } from "@/lib/types";

/** Matches a link against a free-text term by code or destination URL — the client-side fallback when server search is unavailable (501). */
function matches(link: Link, term: string): boolean {
  const q = term.trim().toLowerCase();
  if (!q) return true;
  return link.code.toLowerCase().includes(q) || link.url.toLowerCase().includes(q);
}

export function Analytics() {
  const t = useT();
  const [term, setTerm] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const [clientMode, setClientMode] = useState(false);

  // Always-loaded base list: the source for the client-side fallback and for
  // browsing before typing a search term (mirrors the Links screen's pattern).
  const base = useLinks();
  const dq = useDebounce(term, 300);
  const usingServerSearch = dq !== "" && !clientMode;
  const search = useLinks(dq, undefined, undefined, undefined, undefined, { enabled: usingServerSearch });

  useEffect(() => {
    if (search.error instanceof ApiError && search.error.status === 501) setClientMode(true);
  }, [search.error]);

  const baseLinks = useMemo(() => base.data?.pages.flatMap((page) => page.links) ?? [], [base.data]);
  const searchLinks = useMemo(() => search.data?.pages.flatMap((page) => page.links) ?? [], [search.data]);

  const results = useMemo(() => {
    if (dq === "") return baseLinks;
    if (clientMode) return baseLinks.filter((link) => matches(link, dq));
    return searchLinks;
  }, [baseLinks, searchLinks, clientMode, dq]);

  const isSearching = usingServerSearch && search.isFetching;
  const noResults = dq !== "" && !isSearching && !base.isPending && results.length === 0;

  // Only the base browse list (no active search term) gets a "load more"
  // affordance — client-side filtering and server-side search already work
  // over what's loaded, and a fetched search page mirrors the base list's
  // pagination anyway via the same `useLinks` hook.
  const activeQuery = usingServerSearch ? search : base;

  return (
    <div className="flex flex-col gap-4">
      <div>
        <h1 className="font-heading text-2xl font-semibold">{t("analytics.heading")}</h1>
      </div>

      <div className="relative max-w-sm">
        <Input
          type="search"
          placeholder={t("analytics.searchPlaceholder")}
          value={term}
          onChange={(e) => setTerm(e.target.value)}
          aria-label={t("analytics.searchAriaLabel")}
        />
        {isSearching && (
          <Loader2
            className="absolute right-2 top-1/2 size-4 -translate-y-1/2 animate-spin text-muted-foreground"
            aria-hidden="true"
          />
        )}
      </div>

      {base.isError && (
        <Card className="border-destructive/30">
          <CardContent className="flex flex-col items-center gap-3 py-8 text-center">
            <AlertTriangle className="size-8 text-destructive" aria-hidden="true" />
            <p className="font-medium">
              {base.error instanceof Error ? base.error.message : t("common.retryHint")}
            </p>
          </CardContent>
        </Card>
      )}

      {noResults && (
        <Card>
          <CardContent className="py-6 text-center text-sm text-muted-foreground">
            {t("analytics.noResults")}
          </CardContent>
        </Card>
      )}

      {!base.isError && results.length > 0 && (
        <ul className="flex flex-col gap-1">
          {results.map((link) => (
            <li key={link.code}>
              <button
                type="button"
                aria-label={`${link.code} — ${link.url}`}
                onClick={() => setSelected(link.code)}
                className={
                  "flex w-full items-center gap-3 rounded-lg border border-border px-3 py-2 text-left text-sm transition-colors hover:bg-accent" +
                  (selected === link.code ? " border-primary bg-accent" : "")
                }
              >
                <Link2 className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
                <span className="font-medium">{link.code}</span>
                <span className="min-w-0 flex-1 truncate text-muted-foreground">{link.url}</span>
              </button>
            </li>
          ))}
        </ul>
      )}

      {activeQuery.hasNextPage && (
        <Button
          variant="outline"
          onClick={() => activeQuery.fetchNextPage()}
          disabled={activeQuery.isFetchingNextPage}
          className="self-center"
        >
          {activeQuery.isFetchingNextPage ? t("common.loadingMore") : t("common.loadMore")}
        </Button>
      )}

      {selected ? (
        <StatsView code={selected} />
      ) : (
        <Card>
          <CardContent className="py-12 text-center text-sm text-muted-foreground">
            {t("analytics.empty")}
          </CardContent>
        </Card>
      )}
    </div>
  );
}
