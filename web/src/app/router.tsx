import { createBrowserRouter, Navigate } from "react-router-dom";
import { AppLinks } from "@/routes/AppLinks";
import { Blocklist } from "@/routes/Blocklist";
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
      { path: "app-links", element: <AppLinks /> },
    ],
  },
  { path: "*", element: <Navigate to="/links" replace /> },
]);
