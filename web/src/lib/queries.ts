import { QueryClient, useInfiniteQuery } from "@tanstack/react-query";
import { api } from "./api";

/**
 * Cliente único do TanStack Query para a aplicação. `retry: false` porque um
 * 401 já é tratado globalmente via `setUnauthorizedHandler` (ver App.tsx) e
 * novas tentativas automáticas não ajudam nesse caso.
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
 * Lista paginada de links via keyset (`after`/`next_after`). Cada página
 * carrega `LINKS_PAGE_SIZE` links; `fetchNextPage` busca a próxima usando o
 * cursor devolvido pela API. A busca da tela de Links é client-side, sobre
 * as páginas já carregadas — não dispara nova página.
 */
export function useLinks() {
  return useInfiniteQuery({
    queryKey: ["links"],
    queryFn: ({ pageParam }) => api.listLinks({ after: pageParam ?? undefined, limit: LINKS_PAGE_SIZE }),
    initialPageParam: null as number | null,
    getNextPageParam: (lastPage) => lastPage.next_after,
  });
}
