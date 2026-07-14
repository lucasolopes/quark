import { createBrowserRouter, Navigate } from "react-router-dom";
import { Blocklist } from "@/routes/Blocklist";
import { Import } from "@/routes/Import";
import { LinkStats } from "@/routes/LinkStats";
import { Links } from "@/routes/Links";
import { Login } from "@/routes/Login";
import { Webhooks } from "@/routes/Webhooks";
import { Tokens } from "@/routes/Tokens";
import { RequireAuth } from "./RequireAuth";
import { Shell } from "./Shell";

export const router = createBrowserRouter([
  { path: "/login", element: <Login /> },
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
      { path: "blocklist", element: <Blocklist /> },
      { path: "webhooks", element: <Webhooks /> },
      { path: "import", element: <Import /> },
      { path: "tokens", element: <Tokens /> },
    ],
  },
  { path: "*", element: <Navigate to="/links" replace /> },
]);
