import type { ReactNode } from "react";
import { Navigate } from "react-router-dom";
import { hasToken } from "@/lib/auth";

/** Guarda de rota: sem token salvo, redireciona para /login. */
export function RequireAuth({ children }: { children: ReactNode }) {
  if (!hasToken()) return <Navigate to="/login" replace />;
  return <>{children}</>;
}
