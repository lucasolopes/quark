import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { I18nProvider } from "@/i18n";

interface WithProvidersOptions {
  /** Wraps `ui` in a `MemoryRouter`. Set to false when the test already brings its own router. Default: true. */
  withRouter?: boolean;
  initialEntries?: string[];
  queryClient?: QueryClient;
}

/**
 * Shared test wrapper: `I18nProvider` (forced to `en` for deterministic assertions) +
 * `QueryClientProvider`, optionally wrapped in a `MemoryRouter`.
 */
export function withProviders(ui: ReactNode, options: WithProvidersOptions = {}): ReactNode {
  const { withRouter = true, initialEntries, queryClient } = options;
  const qc = queryClient ?? new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const content = withRouter ? <MemoryRouter initialEntries={initialEntries}>{ui}</MemoryRouter> : ui;

  return (
    <I18nProvider locale="en">
      <QueryClientProvider client={qc}>{content}</QueryClientProvider>
    </I18nProvider>
  );
}
