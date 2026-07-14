import { createBrowserRouter, Navigate } from "react-router-dom";
import { Blocklist } from "@/routes/Blocklist";
import { Import } from "@/routes/Import";
import { LinkStats } from "@/routes/LinkStats";
import { Links } from "@/routes/Links";
import { Login } from "@/routes/Login";
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
      { path: "import", element: <Import /> },
    ],
  },
  { path: "*", element: <Navigate to="/links" replace /> },
]);
