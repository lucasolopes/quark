import { useEffect } from "react";
import { QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "react-router-dom";
import { toast } from "sonner";
import { Toaster } from "@/components/ui/sonner";
import { I18nProvider } from "@/i18n";
import { getMessage, interpolate, MESSAGES, resolveDefaultLocale } from "@/i18n/shared";
import { setPlanLimitHandler, setUnauthorizedHandler } from "@/lib/api";
import { clearToken } from "@/lib/auth";
import { queryClient } from "@/lib/queries";
import { router } from "./router";
import { ThemeProvider } from "./theme";

export function App() {
  useEffect(() => {
    setUnauthorizedHandler(() => {
      clearToken();
      void router.navigate("/login");
    });
  }, []);

  useEffect(() => {
    // This handler is registered outside the I18nProvider tree, so it can't
    // use `useT()`. It resolves the current locale the same way the provider
    // does (`resolveDefaultLocale`) each time it fires, rather than once at
    // mount, so a language switch mid-session is still respected.
    setPlanLimitHandler((b) => {
      const messages = MESSAGES[resolveDefaultLocale()];
      toast.error(interpolate(getMessage(messages, "billing.limitToast"), { limit: b.limit }), {
        action: {
          label: getMessage(messages, "billing.limitToastCta"),
          onClick: () => void router.navigate(`/settings/billing?highlight=${b.upgrade_to}`),
        },
      });
    });
  }, []);

  return (
    <I18nProvider>
      <ThemeProvider>
        <QueryClientProvider client={queryClient}>
          <RouterProvider router={router} />
          <Toaster />
        </QueryClientProvider>
      </ThemeProvider>
    </I18nProvider>
  );
}
