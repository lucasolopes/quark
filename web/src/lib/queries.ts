import { QueryClient, useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "./api";
import type { CreateLinkRequest, PatchLinkRequest } from "./types";

const LINKS_QUERY_KEY = ["links"];
const BLOCKLIST_QUERY_KEY = ["blocklist"];

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
    queryKey: LINKS_QUERY_KEY,
    queryFn: ({ pageParam }) => api.listLinks({ after: pageParam ?? undefined, limit: LINKS_PAGE_SIZE }),
    initialPageParam: null as number | null,
    // O backend sempre manda `next_after` = id do último link da página,
    // mesmo quando ela veio incompleta (não manda `null` só porque acabou).
    // Sem esse corte por tamanho, "Carregar mais" dispararia um fetch extra
    // que sempre volta vazio depois da última página real.
    getNextPageParam: (lastPage) => (lastPage.links.length < LINKS_PAGE_SIZE ? undefined : lastPage.next_after),
  });
}

/** Cria um link; sucesso invalida `useLinks` para refletir o novo registro na lista. */
export function useCreateLink() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (body: CreateLinkRequest) => api.createLink(body),
    onSuccess: () => { void client.invalidateQueries({ queryKey: LINKS_QUERY_KEY }); },
  });
}

/** Atualiza url e/ou ttl de um link existente; sucesso invalida `useLinks`. */
export function usePatchLink() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({ code, body }: { code: string; body: PatchLinkRequest }) => api.patchLink(code, body),
    onSuccess: () => { void client.invalidateQueries({ queryKey: LINKS_QUERY_KEY }); },
  });
}

/** Remove um link; sucesso invalida `useLinks`. */
export function useDeleteLink() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (code: string) => api.deleteLink(code),
    onSuccess: () => { void client.invalidateQueries({ queryKey: LINKS_QUERY_KEY }); },
  });
}

/** Estatísticas agregadas + eventos recentes de um link, para a tela de detalhe. */
export function useStats(code: string) {
  return useQuery({
    queryKey: ["stats", code],
    queryFn: () => api.getStats(code),
    enabled: Boolean(code),
  });
}

/** Lista de domínios bloqueados, para a tela de Blocklist. */
export function useBlocklist() {
  return useQuery({
    queryKey: BLOCKLIST_QUERY_KEY,
    queryFn: () => api.listBlocked(),
  });
}

/** Adiciona um domínio à blocklist; sucesso invalida `useBlocklist`. */
export function useAddBlocked() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (domain: string) => api.addBlocked(domain),
    onSuccess: () => { void client.invalidateQueries({ queryKey: BLOCKLIST_QUERY_KEY }); },
  });
}

/** Remove um domínio da blocklist; sucesso invalida `useBlocklist`. */
export function useRemoveBlocked() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (domain: string) => api.removeBlocked(domain),
    onSuccess: () => { void client.invalidateQueries({ queryKey: BLOCKLIST_QUERY_KEY }); },
  });
}
