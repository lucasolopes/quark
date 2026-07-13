import { QueryClient } from "@tanstack/react-query";

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
