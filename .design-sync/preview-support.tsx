// design-sync preview support — context wrapper for the preview cards.
// The quark panel is dark-first; previews render in the dark theme (`dark`
// class on <html>), with i18n pinned to "en" for determinism and
// MemoryRouter + QueryClient for components that use <Link>/useQuery.
import { useEffect, type ReactNode } from "react";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nProvider } from "../web/src/i18n";

const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false } },
});

export function PreviewProviders({ children }: { children: ReactNode }) {
  useEffect(() => {
    document.documentElement.classList.add("dark");
  }, []);
  return (
    <QueryClientProvider client={queryClient}>
      <MemoryRouter>
        <I18nProvider locale="en">
          {/* Paint the dark ink surface: element screenshots have no page
              background, so the provider owns it. The :has() guard hides the
              wrapper when the component renders nothing, so the floor card's
              empty-root fallback still fires for unauthored previews. */}
          <div className="dark inline-block min-w-full bg-background p-5 text-foreground [&:not(:has(*))]:hidden">
            {children}
          </div>
        </I18nProvider>
      </MemoryRouter>
    </QueryClientProvider>
  );
}
