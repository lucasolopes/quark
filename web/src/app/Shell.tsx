import { BarChart3, Blocks, Fingerprint, Globe, KeyRound, Link2, LogOut, Moon, Plus, Radio, Search, ShieldCheck, Smartphone, Sun, Upload, Users, Webhook } from "lucide-react";
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

/**
 * Derives 1-2 uppercase initials for the sidebar avatar from the signed-in
 * principal's display label (`/admin/me`'s `display`: email or name,
 * whichever the session carries — see `src/auth.rs::Session::display`).
 * One letter for a single-token label (an email, or a one-word name), two
 * for a "First Last" display name.
 */
function initialsFrom(display: string | undefined): string {
  const words = (display ?? "").trim().split(/\s+/).filter(Boolean);
  if (words.length === 0) return "";
  if (words.length === 1) return words[0].slice(0, 1).toUpperCase();
  return (words[0].slice(0, 1) + words[1].slice(0, 1)).toUpperCase();
}

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
        { to: "/domains", label: t("shell.navDomains"), icon: Globe, show: has("full") },
        { to: "/app-links", label: t("shell.navAppLinks"), icon: Smartphone, show: has("full") },
        ...(canManageMembers ? [{ to: "/members", label: t("shell.navMembers"), icon: Users, show: true }] : []),
        ...(canManageSsoDomains ? [{ to: "/sso-provider", label: t("shell.navSsoProvider"), icon: Fingerprint, show: true }] : []),
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

  /** Global link search (topbar): Enter hands the term to the Links screen via `?q=`. */
  function handleSearchKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (e.key !== "Enter") return;
    const term = e.currentTarget.value.trim();
    if (term) navigate(`/links?q=${encodeURIComponent(term)}`);
  }

  return (
    <div className="flex min-h-svh">
      <aside className="flex w-16 shrink-0 flex-col border-r border-sidebar-border bg-sidebar px-3 py-4 sm:w-[250px]">
        <div className="flex items-center justify-center gap-2.5 pb-4 sm:justify-start">
          <QuarkMark className="size-[26px] text-primary drop-shadow-[0_0_8px_rgba(198,249,78,0.55)]" />
          <span className="hidden font-heading text-lg font-bold tracking-tight text-sidebar-foreground sm:inline">
            quark
          </span>
        </div>

        {/* WorkspaceSwitcher moved here from the topbar (cloud only — it renders nothing in OSS). */}
        {me.data?.memberships !== undefined && (
          <div className="hidden pb-4 sm:block">
            <WorkspaceSwitcher />
          </div>
        )}

        <nav className="flex flex-col gap-4">
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
                      "flex items-center justify-center gap-3 rounded-[9px] px-3 py-2 text-[14.5px] font-medium transition-colors sm:justify-start",
                      isActive
                        ? "bg-sidebar-accent text-sidebar-accent-foreground"
                        : "text-sidebar-foreground/70 hover:bg-surface-hover",
                    )
                  }
                >
                  <Icon className="size-[18px] shrink-0" aria-hidden="true" />
                  <span className="hidden sm:inline">{label}</span>
                </NavLink>
              ))}
            </div>
          ))}
        </nav>

        <div className="flex-1" />

        <div
          className="hidden items-center gap-2 pb-3 font-mono text-[11px] text-sidebar-foreground/45 sm:flex"
          title={apiHost}
        >
          <span className="size-1.5 shrink-0 animate-pulse rounded-full bg-primary" aria-hidden="true" />
          <span className="truncate">
            {t("shell.connected")} · {apiHost}
          </span>
        </div>

        {/* User card: avatar + name (from /admin/me's `display`) + logout — moved here from the topbar. */}
        <div className="flex flex-col items-center gap-2.5 border-t border-sidebar-border pt-3 sm:flex-row">
          <div className="flex size-[30px] shrink-0 items-center justify-center rounded-full bg-primary font-heading text-xs font-bold text-primary-foreground">
            {initialsFrom(me.data?.display)}
          </div>
          <div className="hidden min-w-0 flex-1 sm:block">
            <div className="truncate text-[13px] font-semibold text-sidebar-foreground">
              {me.data?.display}
            </div>
          </div>
          <Button variant="ghost" size="icon" aria-label={t("shell.logout")} onClick={handleLogout}>
            <LogOut className="size-4" aria-hidden="true" />
          </Button>
        </div>
      </aside>
      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex h-[62px] shrink-0 items-center justify-between gap-3 border-b border-border px-6">
          <div className="flex max-w-[440px] flex-1 items-center gap-2 rounded-[10px] border border-border bg-secondary px-3.5 focus-within:border-ring focus-within:ring-3 focus-within:ring-ring/50">
            <Search className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
            <input
              type="text"
              placeholder={t("shell.searchPlaceholder")}
              aria-label={t("shell.searchPlaceholder")}
              onKeyDown={handleSearchKeyDown}
              className="w-full min-w-0 flex-1 bg-transparent py-2 text-sm text-foreground outline-none placeholder:text-muted-foreground"
            />
          </div>
          <div className="flex items-center gap-2.5">
            <LanguageSwitcher className="font-mono" />
            <Button
              variant="ghost"
              size="icon"
              aria-label={isDark ? t("shell.themeToLight") : t("shell.themeToDark")}
              onClick={toggle}
            >
              {isDark ? <Sun className="size-4" /> : <Moon className="size-4" />}
            </Button>
            <Button onClick={() => navigate("/links?new=1")}>
              <Plus className="size-4" aria-hidden="true" />
              {t("shell.newLink")}
            </Button>
          </div>
        </header>
        <main className="min-w-0 flex-1 overflow-auto p-6 sm:p-[26px_30px]">
          <Outlet />
        </main>
      </div>
    </div>
  );
}
