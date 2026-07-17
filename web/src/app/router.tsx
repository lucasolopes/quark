import { createBrowserRouter, Navigate } from "react-router-dom";
import { AcceptInvite } from "@/routes/AcceptInvite";
import { Analytics } from "@/routes/Analytics";
import { AppLinks } from "@/routes/AppLinks";
import { Extensions } from "@/routes/Extensions";
import { Import } from "@/routes/Import";
import { LinkStats } from "@/routes/LinkStats";
import { Links } from "@/routes/Links";
import { Login } from "@/routes/Login";
import { Members } from "@/routes/Members";
import { SsoDomains } from "@/routes/SsoDomains";
import { Webhooks } from "@/routes/Webhooks";
import { Tokens } from "@/routes/Tokens";
import { Pixels } from "@/routes/Pixels";
import { RequireAuth } from "./RequireAuth";
import { Shell } from "./Shell";

export const router = createBrowserRouter([
  { path: "/login", element: <Login /> },
  // Public, outside RequireAuth: an invitee has no workspace yet, so nesting
  // this under the authed tree would trap them in WorkspaceGate/onboarding.
  { path: "/invite/:token", element: <AcceptInvite /> },
  {
    path: "/",
    element: (
      <RequireAuth>
        <Shell />
      </RequireAuth>
    ),
    children: [
      { index: true, element: <Navigate to="/links" replace /> },
      { path: "links", element: <Links /> },
      { path: "links/:code", element: <LinkStats /> },
      { path: "webhooks", element: <Webhooks /> },
      { path: "extensions", element: <Extensions /> },
      { path: "import", element: <Import /> },
      { path: "tokens", element: <Tokens /> },
      { path: "pixels", element: <Pixels /> },
      { path: "analytics", element: <Analytics /> },
      { path: "members", element: <Members /> },
      { path: "sso-domains", element: <SsoDomains /> },
      { path: "app-links", element: <AppLinks /> },
    ],
  },
  { path: "*", element: <Navigate to="/links" replace /> },
]);
