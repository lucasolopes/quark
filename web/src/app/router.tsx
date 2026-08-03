import { lazy } from "react";
import { createBrowserRouter, Navigate } from "react-router-dom";
import { eePublicRoutes, eeRoutes } from "@ee";
import { RequireAuth } from "./RequireAuth";
import { suspended } from "./suspended";
import { Shell } from "./Shell";

// Page-level route components are code-split so they land in their own chunks
// instead of the main bundle. The layout (Shell) and auth guard (RequireAuth)
// stay eager: they wrap every authed route and are always needed immediately.
const Analytics = lazy(() => import("@/routes/Analytics").then((m) => ({ default: m.Analytics })));
const AppLinks = lazy(() => import("@/routes/AppLinks").then((m) => ({ default: m.AppLinks })));
const Extensions = lazy(() => import("@/routes/Extensions").then((m) => ({ default: m.Extensions })));
const ExtensionDetail = lazy(() => import("@/routes/ExtensionDetail").then((m) => ({ default: m.ExtensionDetail })));
const Import = lazy(() => import("@/routes/Import").then((m) => ({ default: m.Import })));
const LinkStats = lazy(() => import("@/routes/LinkStats").then((m) => ({ default: m.LinkStats })));
const Links = lazy(() => import("@/routes/Links").then((m) => ({ default: m.Links })));
const Login = lazy(() => import("@/routes/Login").then((m) => ({ default: m.Login })));
const Webhooks = lazy(() => import("@/routes/Webhooks").then((m) => ({ default: m.Webhooks })));
const Tokens = lazy(() => import("@/routes/Tokens").then((m) => ({ default: m.Tokens })));
const Pixels = lazy(() => import("@/routes/Pixels").then((m) => ({ default: m.Pixels })));

/** Centered spinner shown while a lazily-loaded route chunk is fetched. */
export const router = createBrowserRouter([
  { path: "/login", element: suspended(<Login />) },
  // Public Enterprise routes (invite acceptance). Empty in Community, where
  // `@ee` resolves to the inert stub.
  ...eePublicRoutes,
  {
    path: "/",
    element: (
      <RequireAuth>
        <Shell />
      </RequireAuth>
    ),
    children: [
      { index: true, element: <Navigate to="/links" replace /> },
      { path: "links", element: suspended(<Links />) },
      { path: "links/:code", element: suspended(<LinkStats />) },
      { path: "webhooks", element: suspended(<Webhooks />) },
      { path: "extensions", element: suspended(<Extensions />) },
      { path: "extensions/:id", element: suspended(<ExtensionDetail />) },
      { path: "import", element: suspended(<Import />) },
      { path: "tokens", element: suspended(<Tokens />) },
      { path: "pixels", element: suspended(<Pixels />) },
      { path: "analytics", element: suspended(<Analytics />) },
      // Enterprise screens (workspaces, invites, per-tenant SSO, domains).
      // Empty in Community.
      ...eeRoutes,
      { path: "app-links", element: suspended(<AppLinks />) },
    ],
  },
  { path: "*", element: <Navigate to="/links" replace /> },
]);
