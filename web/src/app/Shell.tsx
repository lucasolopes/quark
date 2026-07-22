import { BarChart3, Blocks, KeyRound, Link2, LogOut, Moon, Radio, ShieldCheck, Smartphone, Sun, Upload, Users, Webhook } from "lucide-react";
import { useTheme } from "next-themes";
import { NavLink, Outlet, useNavigate } from "react-router-dom";
import { QuarkMark } from "@/components/brand/QuarkMark";
import { LanguageSwitcher } from "@/components/LanguageSwitcher";
import { Button } from "@/components/ui/button";
import { WorkspaceSwitcher } from "@/components/WorkspaceSwitcher";
import { useT } from "@/i18n";
import { api } from "@/lib/api";
import { clearToken } from "@/lib/auth";
import { useMe } from "@/lib/queries";
import { useScopes } from "@/lib/scopes";
import { cn } from "@/lib/utils";

/** Roles that can manage the workspace's team (create/revoke invites). */
const MEMBERS_MANAGER_ROLES = new Set(["owner", "admin"]);

export function Shell() {
  const t = useT();
  const navigate = useNavigate();
  const { resolvedTheme, setTheme } = useTheme();
  const isDark = resolvedTheme === "dark";
  const toggle = () => setTheme(isDark ? "light" : "dark");
  const me = useMe();
  const { has } = useScopes();

  // Members and SSO domains are cloud-only (`memberships` absent in OSS) and
  // restricted to the current tenant's Owner/Admin — Member/Viewer never
  // sees either nav item.
  const currentRole = me.data?.memberships?.find((m) => m.tenant_id === me.data?.current_tenant)?.role;
  const canManageMembers = me.data?.memberships !== undefined && currentRole != null && MEMBERS_MANAGER_ROLES.has(currentRole);
  const canManageSsoDomains = canManageMembers;

  // Each item declares the scope it needs; `has` hides the ones the current
  // principal cannot use (Viewer has no `links_write`, Member/Viewer have no
  // `full`), so the nav never points at a page that would only 403. `full`
  // covers everything (see `useScopes`), so an admin/token sees all of them.
  const navGroups = [
    {
      label: t("shell.navGroupLinks"),
      items: [
        { to: "/links", label: t("shell.navLinks"), icon: Link2, show: has("links_read") },
        { to: "/import", label: t("shell.navImport"), icon: Upload, show: has("links_write") },
      ],
    },
    {
      label: t("shell.navGroupData"),
      items: [
        { to: "/analytics", label: t("shell.navAnalytics"), icon: BarChart3, show: has("analytics") },
        { to: "/pixels", label: t("shell.navPixels"), icon: Radio, show: has("analytics") },
      ],
    },
    {
      label: t("shell.navGroupAuto"),
      items: [
        { to: "/webhooks", label: t("shell.navWebhooks"), icon: Webhook, show: has("webhooks") },
        { to: "/extensions", label: t("shell.navExtensions"), icon: Blocks, show: has("full") },
      ],
    },
    {
      label: t("shell.navGroupDev"),
      items: [
        { to: "/tokens", label: t("shell.navTokens"), icon: KeyRound, show: has("full") },
        { to: "/app-links", label: t("shell.navAppLinks"), icon: Smartphone, show: has("full") },
        ...(canManageMembers ? [{ to: "/members", label: t("shell.navMembers"), icon: Users, show: true }] : []),
        ...(canManageSsoDomains ? [{ to: "/sso-domains", label: t("shell.navSsoDomains"), icon: ShieldCheck, show: true }] : []),
      ],
    },
  ]
    .map((group) => ({ ...group, items: group.items.filter((item) => item.show) }))
    .filter((group) => group.items.length > 0);

  async function handleLogout() {
    clearToken();
    // Revoke the OIDC session server-side (no-op if it was a token login). When
    // the server hands back an end-session URL, do a top-level navigation to it
    // so the IdP session is ended too, not just quark's (RP-initiated logout,
    // LUC-79). Fall back to /login on any error or when there is no URL.
    try {
      const { logout_url } = await api.logout();
      window.location.href = logout_url ?? "/login";
    } catch {
      navigate("/login", { replace: true });
    }
  }

  const apiHost = (
    (import.meta.env.VITE_API_BASE_URL as string | undefined) || window.location.origin
  )
    .replace(/^https?:\/\//, "")
    .replace(/\/+$/, "");

  return (
    <div className="flex min-h-svh">
      <aside className="flex w-16 shrink-0 flex-col border-r border-sidebar-border bg-sidebar sm:w-56">
        <div className="flex h-14 items-center justify-center gap-2.5 px-2 sm:justify-start sm:px-4">
          <QuarkMark className="size-6 text-primary drop-shadow-[0_0_8px_rgba(198,249,78,0.55)]" />
          <span className="hidden font-heading text-lg font-bold tracking-tight text-sidebar-foreground sm:inline">
            quark
          </span>
        </div>
        <nav className="flex flex-col gap-4 px-2 py-2">
          {navGroups.map((group) => (
            <div key={group.label} className="flex flex-col gap-1">
              <div className="hidden px-3 pb-1 font-mono text-[10px] font-medium tracking-[0.14em] text-sidebar-foreground/45 uppercase sm:block">
                {group.label}
              </div>
              {group.items.map(({ to, label, icon: Icon }) => (
                <NavLink
                  key={to}
                  to={to}
                  title={label}
                  className={({ isActive }) =>
                    cn(
                      "flex items-center justify-center gap-2 rounded-lg px-3 py-2 text-sm font-medium text-sidebar-foreground/70 transition-colors hover:bg-sidebar-accent hover:text-sidebar-accent-foreground sm:justify-start",
                      isActive && "bg-sidebar-accent text-sidebar-accent-foreground",
                    )
                  }
                >
                  <Icon className="size-4 shrink-0" aria-hidden="true" />
                  <span className="hidden sm:inline">{label}</span>
                </NavLink>
              ))}
            </div>
          ))}
        </nav>
        <div
          className="mt-auto hidden items-center gap-2 px-3 py-3 font-mono text-[11px] text-sidebar-foreground/45 sm:flex"
          title={apiHost}
        >
          <span className="size-1.5 shrink-0 animate-pulse rounded-full bg-primary" aria-hidden="true" />
          <span className="truncate">
            {t("shell.connected")} · {apiHost}
          </span>
        </div>
      </aside>
      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex h-14 shrink-0 items-center gap-2 border-b border-border px-4">
          <WorkspaceSwitcher />
          <div className="flex-1" />
          <LanguageSwitcher />
          <Button
            variant="ghost"
            size="icon"
            aria-label={isDark ? t("shell.themeToLight") : t("shell.themeToDark")}
            onClick={toggle}
          >
            {isDark ? <Sun className="size-4" /> : <Moon className="size-4" />}
          </Button>
          <Button variant="outline" size="sm" onClick={handleLogout}>
            <LogOut className="size-4" />
            {t("shell.logout")}
          </Button>
        </header>
        <main className="min-w-0 flex-1 overflow-auto p-4 sm:p-6">
          <Outlet />
        </main>
      </div>
    </div>
  );
}
