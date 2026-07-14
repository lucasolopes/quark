import { QueryClient, useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "./api";
import type { CreateLinkRequest, CreatePixelRequest, CreateTokenRequest, CreateWebhookRequest, PatchLinkRequest, PatchWebhookRequest, WellknownName } from "./types";

const LINKS_QUERY_KEY = ["links"];
const WEBHOOKS_QUERY_KEY = ["webhooks"];
const TAGS_QUERY_KEY = ["tags"];
const TOKENS_QUERY_KEY = ["tokens"];
const PIXELS_QUERY_KEY = ["pixels"];

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
 *
 * `tag` filters the list server-side (`GET /admin/links?tag=`); it's part of
 * the query key alongside `q` so switching the tag filter refetches instead
 * of reusing a stale cache entry.
 */
export function useLinks(q?: string, tag?: string, options: { enabled?: boolean } = {}) {
  const term = q?.trim() ?? "";
  const tagTerm = tag?.trim() ?? "";
  return useInfiniteQuery({
    queryKey: [...LINKS_QUERY_KEY, term, tagTerm],
    queryFn: ({ pageParam }) =>
      api.listLinks({
        after: pageParam ?? undefined,
        limit: LINKS_PAGE_SIZE,
        q: term || undefined,
        tag: tagTerm || undefined,
      }),
    initialPageParam: null as number | null,
    getNextPageParam: (lastPage) => (lastPage.links.length < LINKS_PAGE_SIZE ? undefined : lastPage.next_after),
    enabled: options.enabled,
  });
}

/** Creates a link; on success invalidates `useLinks` and `useTags` (a new tag may now exist). */
export function useCreateLink() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (body: CreateLinkRequest) => api.createLink(body),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: LINKS_QUERY_KEY });
      void client.invalidateQueries({ queryKey: TAGS_QUERY_KEY });
    },
  });
}

/** Updates url/ttl/tags of an existing link; on success invalidates `useLinks` and `useTags`. */
export function usePatchLink() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({ code, body }: { code: string; body: PatchLinkRequest }) => api.patchLink(code, body),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: LINKS_QUERY_KEY });
      void client.invalidateQueries({ queryKey: TAGS_QUERY_KEY });
    },
  });
}

/** Deletes a link; on success invalidates `useLinks` and `useTags` (its tags may no longer be in use). */
export function useDeleteLink() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (code: string) => api.deleteLink(code),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: LINKS_QUERY_KEY });
      void client.invalidateQueries({ queryKey: TAGS_QUERY_KEY });
    },
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

/** Distinct set of tags in use across all links, for the Links screen's tag filter. */
export function useTags() {
  return useQuery({
    queryKey: TAGS_QUERY_KEY,
    queryFn: () => api.listTags(),
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

/** Bulk-imports links from a raw CSV/JSON body; on success invalidates `useLinks`. */
export function useImport() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({ body, contentType }: { body: string; contentType: string }) => api.importLinks(body, contentType),
    onSuccess: () => { void client.invalidateQueries({ queryKey: LINKS_QUERY_KEY }); },
  });
}

/** List of API tokens, for the Tokens screen. Never includes the hash or plaintext. */
export function useTokens() {
  return useQuery({
    queryKey: TOKENS_QUERY_KEY,
    queryFn: () => api.listTokens(),
  });
}

/** Creates an API token; on success invalidates `useTokens`. The response carries the plaintext once. */
export function useCreateToken() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (body: CreateTokenRequest) => api.createToken(body),
    onSuccess: () => { void client.invalidateQueries({ queryKey: TOKENS_QUERY_KEY }); },
  });
}

/** Revokes (deletes) an API token; on success invalidates `useTokens`. */
export function useDeleteToken() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (id: number) => api.deleteToken(id),
    onSuccess: () => { void client.invalidateQueries({ queryKey: TOKENS_QUERY_KEY }); },
  });
}

/** List of configured conversion-forwarding pixels, for the Pixels screen. */
export function usePixels() {
  return useQuery({
    queryKey: PIXELS_QUERY_KEY,
    queryFn: () => api.listPixels(),
  });
}

/** Creates a pixel config; on success invalidates `usePixels`. */
export function useCreatePixel() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (body: CreatePixelRequest) => api.createPixel(body),
    onSuccess: () => { void client.invalidateQueries({ queryKey: PIXELS_QUERY_KEY }); },
  });
}

/** Deletes a pixel config; on success invalidates `usePixels`. */
export function useDeletePixel() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (id: number) => api.deletePixel(id),
    onSuccess: () => { void client.invalidateQueries({ queryKey: PIXELS_QUERY_KEY }); },
  });
}

const wellknownKey = (name: WellknownName) => ["wellknown", name];

/** Current body of a well-known app-association document (`null` when unset). */
export function useWellknown(name: WellknownName) {
  return useQuery({
    queryKey: wellknownKey(name),
    queryFn: () => api.getWellknown(name),
  });
}

/** Stores a well-known document; on success invalidates its query. */
export function usePutWellknown(name: WellknownName) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (body: string) => api.putWellknown(name, body),
    onSuccess: () => { void client.invalidateQueries({ queryKey: wellknownKey(name) }); },
  });
}

/** Removes a well-known document; on success invalidates its query. */
export function useDeleteWellknown(name: WellknownName) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: () => api.deleteWellknown(name),
    onSuccess: () => { void client.invalidateQueries({ queryKey: wellknownKey(name) }); },
  });
}
