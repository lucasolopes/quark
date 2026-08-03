// Inert implementation of the Enterprise surface, used by the Community
// edition.
//
// This file is AGPL, like the rest of `web/src/` outside `web/src/ee/`. It
// exists so the panel keeps building and running when `web/src/ee/` is not
// there (LUC-19). `vite.config.ts` points the `@ee` alias here when the EE
// directory is missing or `VITE_QUARK_EE` is off, and `tsconfig` points here
// always, which makes this file the type contract the real implementation has
// to satisfy.
//
// None of these components ever render in the Community edition: the server
// does not run in cloud mode, so `me.multi_tenant` is false and the screens
// that depend on it are never reached.
import type { RouteObject } from "react-router";
import type { MeResponse } from "@/lib/types";

/** True only in the Enterprise edition. */
export const eeEnabled = false;

/** Enterprise routes mounted by the router. Empty here. */
export const eeRoutes: RouteObject[] = [];

/** Public invite-acceptance route, outside the authenticated tree. */
export const eePublicRoutes: RouteObject[] = [];

/** Workspace onboarding. Unreachable without cloud mode. */
export function WorkspaceGate(_props: { me: MeResponse }) {
  return null;
}

/** Workspace switcher at the top of the sidebar. */
export function WorkspaceSwitcher() {
  return null;
}
