import { Loader2 } from "lucide-react";
import type { ReactNode } from "react";
import { Navigate } from "react-router-dom";
import { useMe } from "@/lib/queries";
import { hasToken } from "@/lib/auth";
import { WorkspaceGate } from "./WorkspaceGate";

/**
 * Route guard. A saved break-glass token authenticates immediately. Otherwise
 * it checks for an OIDC login session (`GET /admin/me`); while that resolves it
 * shows a spinner, and with no token and no session it redirects to /login.
 * In cloud mode, an authenticated user with no current workspace is routed
 * through `WorkspaceGate` (onboarding / auto-switch) instead of the app.
 */
export function RequireAuth({ children }: { children: ReactNode }) {
  const tokenPresent = hasToken();
  const me = useMe();

  if (tokenPresent) return <>{children}</>;
  if (me.isLoading) {
    return (
      <div className="flex min-h-svh items-center justify-center bg-background">
        <Loader2 className="size-6 animate-spin text-muted-foreground" aria-label="Loading" />
      </div>
    );
  }
  if (!me.data?.authenticated) return <Navigate to="/login" replace />;
  // Cloud mode only (OSS omits `memberships`): gate on having a current workspace.
  const cloud = me.data.memberships !== undefined;
  if (cloud && me.data.current_tenant == null) return <WorkspaceGate me={me.data} />;
  return <>{children}</>;
}
