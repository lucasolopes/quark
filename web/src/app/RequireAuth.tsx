import type { ReactNode } from "react";
import { Navigate } from "react-router-dom";
import { hasToken } from "@/lib/auth";

/** Route guard: with no saved token, redirects to /login. */
export function RequireAuth({ children }: { children: ReactNode }) {
  if (!hasToken()) return <Navigate to="/login" replace />;
  return <>{children}</>;
}
