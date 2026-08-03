// quark Enterprise Edition, panel.
//
// WARNING: this directory is NOT AGPL. It is covered by the quark Enterprise
// Edition License in `web/src/ee/LICENSE`. Everything else in `web/` is
// AGPL-3.0-only. See `docs/LICENSING.md`.
//
// The barrel the core imports through the `@ee` alias. `vite.config.ts`
// resolves that alias here when `VITE_QUARK_EE` is on and the directory
// exists, and to `@/lib/ee-stub` otherwise. Both expose the same shape, and the
// stub is what `tsconfig` typechecks the core against.
import { lazy } from "react";
import type { RouteObject } from "react-router";

import { suspended } from "@/app/suspended";

export { WorkspaceGate } from "./WorkspaceGate";
export { WorkspaceSwitcher } from "./WorkspaceSwitcher";

const AcceptInvite = lazy(() => import("./AcceptInvite").then((m) => ({ default: m.AcceptInvite })));
const Members = lazy(() => import("./Members").then((m) => ({ default: m.Members })));
const SsoDomains = lazy(() => import("./SsoDomains").then((m) => ({ default: m.SsoDomains })));
const OidcProvider = lazy(() => import("./OidcProvider").then((m) => ({ default: m.OidcProvider })));
const Domains = lazy(() => import("./Domains").then((m) => ({ default: m.Domains })));

export const eeEnabled = true;

/** Enterprise screens inside the authenticated tree, mounted by `router.tsx`. */
export const eeRoutes: RouteObject[] = [
  { path: "members", element: suspended(<Members />) },
  { path: "sso-domains", element: suspended(<SsoDomains />) },
  { path: "sso-provider", element: suspended(<OidcProvider />) },
  { path: "domains", element: suspended(<Domains />) },
];

/**
 * Invite acceptance. Outside the authenticated tree on purpose: someone
 * arriving from an invite link has no workspace yet, and mounting this under
 * the authenticated tree would trap them in `WorkspaceGate`.
 */
export const eePublicRoutes: RouteObject[] = [
  { path: "/invite/:token", element: suspended(<AcceptInvite />) },
];
