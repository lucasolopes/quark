import { QueryClient, useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "./api";
import type { CreateLinkRequest, CreateWebhookRequest, PatchLinkRequest, PatchWebhookRequest } from "./types";

const LINKS_QUERY_KEY = ["links"];
const BLOCKLIST_QUERY_KEY = ["blocklist"];
const WEBHOOKS_QUERY_KEY = ["webhooks"];

/**
 * The application's single TanStack Query client. `retry: false` because a
 * 401 is already handled globally via `setUnauthorizedHandler` (see App.tsx)
 * and automatic retries don't help in that case.
 */
export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: false,
      refetchOnWindowFocus: false,
    },
  },
});

const LINKS_PAGE_SIZE = 50;

/**
 * Paginated link list via keyset (`after`/`next_after`). Each page loads
 * `LINKS_PAGE_SIZE` links; `fetchNextPage` fetches the next one using the
 * cursor returned by the API.
 *
 * Without `q`, this is the base list (always loaded — the source for the
 * Links screen's client-side fallback). With `q`, it's the paginated
 * server-side search; the backend may respond with 501 (search not
 * supported), in which case the screen falls back to filtering client-side
 * over the base list. Doesn't set its own `retry` — inherits the global
 * `retry: false` from `queryClient` (same reason as that comment: a 401 is
 * already handled by `onUnauthorized`, and automatic retries don't help on
 * 501 either, which is a final response, not a transient error). A custom
 * `retry` here would leak into the call without `q` too, since it's the same
 * hook.
 */
export function useLinks(q?: string, options: { enabled?: boolean } = {}) {
  const term = q?.trim() ?? "";
  return useInfiniteQuery({
    queryKey: [...LINKS_QUERY_KEY, term],
    queryFn: ({ pageParam }) =>
      api.listLinks({ after: pageParam ?? undefined, limit: LINKS_PAGE_SIZE, q: term || undefined }),
    initialPageParam: null as number | null,
    getNextPageParam: (lastPage) => (lastPage.links.length < LINKS_PAGE_SIZE ? undefined : lastPage.next_after),
    enabled: options.enabled,
  });
}

/** Creates a link; on success invalidates `useLinks` to reflect the new record in the list. */
export function useCreateLink() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (body: CreateLinkRequest) => api.createLink(body),
    onSuccess: () => { void client.invalidateQueries({ queryKey: LINKS_QUERY_KEY }); },
  });
}

/** Updates url and/or ttl of an existing link; on success invalidates `useLinks`. */
export function usePatchLink() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({ code, body }: { code: string; body: PatchLinkRequest }) => api.patchLink(code, body),
    onSuccess: () => { void client.invalidateQueries({ queryKey: LINKS_QUERY_KEY }); },
  });
}

/** Deletes a link; on success invalidates `useLinks`. */
export function useDeleteLink() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (code: string) => api.deleteLink(code),
    onSuccess: () => { void client.invalidateQueries({ queryKey: LINKS_QUERY_KEY }); },
  });
}

/** Aggregated stats + recent events for a link, for the detail screen. */
export function useStats(code: string) {
  return useQuery({
    queryKey: ["stats", code],
    queryFn: () => api.getStats(code),
    enabled: Boolean(code),
  });
}

/** List of blocked domains, for the Blocklist screen. */
export function useBlocklist() {
  return useQuery({
    queryKey: BLOCKLIST_QUERY_KEY,
    queryFn: () => api.listBlocked(),
  });
}

/** Adds a domain to the blocklist; on success invalidates `useBlocklist`. */
export function useAddBlocked() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (domain: string) => api.addBlocked(domain),
    onSuccess: () => { void client.invalidateQueries({ queryKey: BLOCKLIST_QUERY_KEY }); },
  });
}

/** Removes a domain from the blocklist; on success invalidates `useBlocklist`. */
export function useRemoveBlocked() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (domain: string) => api.removeBlocked(domain),
    onSuccess: () => { void client.invalidateQueries({ queryKey: BLOCKLIST_QUERY_KEY }); },
  });
}

/** List of registered webhook subscriptions, for the Webhooks screen. */
export function useWebhooks() {
  return useQuery({
    queryKey: WEBHOOKS_QUERY_KEY,
    queryFn: () => api.listWebhooks(),
  });
}

/** Creates a webhook subscription; on success invalidates `useWebhooks`. Response carries the raw secret once. */
export function useCreateWebhook() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (body: CreateWebhookRequest) => api.createWebhook(body),
    onSuccess: () => { void client.invalidateQueries({ queryKey: WEBHOOKS_QUERY_KEY }); },
  });
}

/** Updates url/events/active of an existing webhook; on success invalidates `useWebhooks`. */
export function usePatchWebhook() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({ id, body }: { id: number; body: PatchWebhookRequest }) => api.patchWebhook(id, body),
    onSuccess: () => { void client.invalidateQueries({ queryKey: WEBHOOKS_QUERY_KEY }); },
  });
}

/** Deletes a webhook subscription; on success invalidates `useWebhooks`. */
export function useDeleteWebhook() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (id: number) => api.deleteWebhook(id),
    onSuccess: () => { void client.invalidateQueries({ queryKey: WEBHOOKS_QUERY_KEY }); },
  });
}

/** Sends a test event to a webhook's endpoint. Doesn't touch the list — it doesn't change server state worth refetching. */
export function useTestWebhook() {
  return useMutation({
    mutationFn: (id: number) => api.testWebhook(id),
  });
}
