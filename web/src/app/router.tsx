import { Loader2 } from "lucide-react";
import { lazy, Suspense, type ReactElement } from "react";
import { createBrowserRouter, Navigate } from "react-router-dom";
import { RequireAuth } from "./RequireAuth";
import { Shell } from "./Shell";

// Page-level route components are code-split so they land in their own chunks
// instead of the main bundle. The layout (Shell) and auth guard (RequireAuth)
// stay eager: they wrap every authed route and are always needed immediately.
const AcceptInvite = lazy(() => import("@/routes/AcceptInvite").then((m) => ({ default: m.AcceptInvite })));
const Analytics = lazy(() => import("@/routes/Analytics").then((m) => ({ default: m.Analytics })));
const AppLinks = lazy(() => import("@/routes/AppLinks").then((m) => ({ default: m.AppLinks })));
const Extensions = lazy(() => import("@/routes/Extensions").then((m) => ({ default: m.Extensions })));
const Import = lazy(() => import("@/routes/Import").then((m) => ({ default: m.Import })));
const LinkStats = lazy(() => import("@/routes/LinkStats").then((m) => ({ default: m.LinkStats })));
const Links = lazy(() => import("@/routes/Links").then((m) => ({ default: m.Links })));
const Login = lazy(() => import("@/routes/Login").then((m) => ({ default: m.Login })));
const Members = lazy(() => import("@/routes/Members").then((m) => ({ default: m.Members })));
const SsoDomains = lazy(() => import("@/routes/SsoDomains").then((m) => ({ default: m.SsoDomains })));
const Webhooks = lazy(() => import("@/routes/Webhooks").then((m) => ({ default: m.Webhooks })));
const Tokens = lazy(() => import("@/routes/Tokens").then((m) => ({ default: m.Tokens })));
const Pixels = lazy(() => import("@/routes/Pixels").then((m) => ({ default: m.Pixels })));

/** Centered spinner shown while a lazily-loaded route chunk is fetched. */
function RouteFallback() {
  return (
    <div className="flex min-h-[60vh] items-center justify-center" aria-hidden="true">
      <Loader2 className="size-6 animate-spin text-muted-foreground" />
    </div>
  );
}

/** Wrap a lazy route element in Suspense so its chunk can load without blocking. */
function suspended(element: ReactElement): ReactElement {
  return <Suspense fallback={<RouteFallback />}>{element}</Suspense>;
}

export const router = createBrowserRouter([
  { path: "/login", element: suspended(<Login />) },
  // Public, outside RequireAuth: an invitee has no workspace yet, so nesting
  // this under the authed tree would trap them in WorkspaceGate/onboarding.
  { path: "/invite/:token", element: suspended(<AcceptInvite />) },
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
      { path: "import", element: suspended(<Import />) },
      { path: "tokens", element: suspended(<Tokens />) },
      { path: "pixels", element: suspended(<Pixels />) },
      { path: "analytics", element: suspended(<Analytics />) },
      { path: "members", element: suspended(<Members />) },
      { path: "sso-domains", element: suspended(<SsoDomains />) },
      { path: "app-links", element: suspended(<AppLinks />) },
    ],
  },
  { path: "*", element: <Navigate to="/links" replace /> },
]);
