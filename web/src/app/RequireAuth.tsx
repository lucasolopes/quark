import { Loader2 } from "lucide-react";
import type { ReactNode } from "react";
import { Navigate } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { hasToken } from "@/lib/auth";

/**
 * Route guard. A saved break-glass token authenticates immediately. Otherwise
 * it checks for an OIDC login session (`GET /admin/me`); while that resolves it
 * shows a spinner, and with no token and no session it redirects to /login.
 */
export function RequireAuth({ children }: { children: ReactNode }) {
  const tokenPresent = hasToken();
  const me = useQuery({
    queryKey: ["me"],
    queryFn: () => api.me(),
    enabled: !tokenPresent,
    retry: false,
    staleTime: 30_000,
  });

  if (tokenPresent) return <>{children}</>;
  if (me.isLoading) {
    return (
      <div className="flex min-h-svh items-center justify-center bg-background">
        <Loader2 className="size-6 animate-spin text-muted-foreground" aria-label="Loading" />
      </div>
    );
  }
  if (me.data?.authenticated) return <>{children}</>;
  return <Navigate to="/login" replace />;
}
