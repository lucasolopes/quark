import { BarChart3, Blocks, Fingerprint, Globe, KeyRound, Link2, LogOut, Menu, Moon, Plus, Radio, Search, ShieldCheck, Smartphone, Sun, Upload, Users, Webhook, XIcon, type LucideIcon } from "lucide-react";
import { useTheme } from "next-themes";
import { useEffect, useRef, useState } from "react";
import { NavLink, Outlet, useLocation, useNavigate, useSearchParams } from "react-router-dom";
import { QuarkMark } from "@/components/brand/QuarkMark";
import { LanguageSwitcher } from "@/components/LanguageSwitcher";
import { MobileNav } from "@/components/MobileNav";
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
 * One item in the sidebar/drawer nav. `show` records whether the signed-in
 * principal's scopes grant it; always `true` by the time it reaches render
 * (`Shell` filters on it below), but kept on the shape so the pre-filter
 * array and the one handed to render share a single type.
 */
export interface NavItem {
  to: string;
  label: string;
  icon: LucideIcon;
  show: boolean;
}

/**
 * A labeled group of nav items. `Shell` builds the RBAC-filtered array ONCE
 * (`navGroups` below) and it is the single source of truth: the desktop
 * sidebar and `MobileNav`'s drawer both render that same array — neither
 * recomputes its own copy.
 */
export interface NavGroup {
  label: string;
  items: NavItem[];
}

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
  const location = useLocation();
  const [searchParams, setSearchParams] = useSearchParams();
  const { resolvedTheme, setTheme } = useTheme();
  const isDark = resolvedTheme === "dark";
  const toggle = () => setTheme(isDark ? "light" : "dark");
  const me = useMe();
  const { has } = useScopes();
  // Single cut at `md` (768px, LUC-96): the icon rail below `sm` is retired —
  // < md the sidebar disappears entirely and this drawer takes over.
  const [mobileNavOpen, setMobileNavOpen] = useState(false);
  // Whether the < md search row (below the topbar) is showing. Desktop's
  // inline search box is unaffected by this.
  const [searchExpanded, setSearchExpanded] = useState(false);

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
  // This is the SOLE computation of the nav — see the `NavGroup` doc comment.
  const navGroups: NavGroup[] = [
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

  // The topbar is the app's ONE search input (mock: appShell.html; the Links
  // screen itself has none — mock: isTabLinks.html). `q` is shared with the
  // Links screen via the URL, so this box behaves differently depending on
  // where the user already is:
  //  - on /links: every keystroke drives the filter live (debounced below).
  //  - elsewhere: typing just fills the box; Enter is what navigates there.
  // Below `md`, the lupa button toggles a second row with the SAME box: both
  // renders share this one value/these handlers, never a second implementation.
  const onLinksScreen = location.pathname === "/links";
  const qParam = searchParams.get("q") ?? "";
  const [searchValue, setSearchValue] = useState(qParam);
  // Holds the in-flight debounce timer for a live `q` push, and the latest
  // `searchParams` so the timer's callback can merge into the current
  // querystring instead of a stale one captured 300ms earlier.
  const pushTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const searchParamsRef = useRef(searchParams);
  searchParamsRef.current = searchParams;

  // Reflect `?q=` into the box whenever it changes for a reason other than
  // typing here: mount, browser back/forward, or landing on /links via a
  // sidebar click or an Enter-navigation from another screen. Cancels any
  // pending debounced push first, so a URL-driven change is never echoed
  // straight back with whatever stale term the box happened to hold.
  useEffect(() => {
    if (pushTimerRef.current) {
      clearTimeout(pushTimerRef.current);
      pushTimerRef.current = null;
    }
    if (!onLinksScreen) return;
    setSearchValue(qParam);
  }, [onLinksScreen, qParam]);

  // Belt-and-suspenders: drop a pending push if Shell itself ever unmounts
  // mid-debounce (it doesn't in the real router — it's the top-level layout —
  // but test trees remount routinely).
  useEffect(() => {
    return () => {
      if (pushTimerRef.current) clearTimeout(pushTimerRef.current);
    };
  }, []);

  function handleSearchChange(e: React.ChangeEvent<HTMLInputElement>) {
    const value = e.target.value;
    setSearchValue(value);
    if (!onLinksScreen) return;
    if (pushTimerRef.current) clearTimeout(pushTimerRef.current);
    pushTimerRef.current = setTimeout(() => {
      pushTimerRef.current = null;
      const next = new URLSearchParams(searchParamsRef.current);
      if (value) next.set("q", value);
      else next.delete("q");
      setSearchParams(next, { replace: true });
    }, 300);
  }

  /**
   * On any screen other than Links, Enter hands the term over via `?q=` (a
   * normal push navigation, so back returns to where the user was). While
   * already on Links, live typing above already keeps `?q=` in sync, so
   * Enter there has nothing left to do.
   */
  function handleSearchKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (e.key !== "Enter" || onLinksScreen) return;
    const term = e.currentTarget.value.trim();
    if (term) navigate(`/links?q=${encodeURIComponent(term)}`);
  }

  // Logo/wordmark + WorkspaceSwitcher (cloud only): the top of the sidebar,
  // reused verbatim as the top of the drawer (`MobileNav`'s `children`).
  const sidebarHeader = (
    <>
      <div className="flex items-center gap-2.5 pb-4">
        <QuarkMark className="size-[26px] text-primary drop-shadow-[0_0_8px_rgba(198,249,78,0.3)]" />
        <span className="font-heading text-lg font-bold tracking-[-0.04em] text-strong">
          quark
        </span>
      </div>
      {me.data?.memberships !== undefined && (
        <div className="pb-4">
          <WorkspaceSwitcher />
        </div>
      )}
    </>
  );

  // "connected · host" status line, reused in both the sidebar and the drawer.
  const connectedLine = (
    <div className="flex items-center gap-2 pb-3 font-mono text-[11px] text-sidebar-foreground/45" title={apiHost}>
      <span className="size-1.5 shrink-0 animate-pulse rounded-full bg-primary" aria-hidden="true" />
      <span className="truncate">
        {t("shell.connected")} · {apiHost}
      </span>
    </div>
  );

  // User card: avatar + name (from /admin/me's `display`) + logout, reused in
  // both the sidebar and the drawer.
  const userCard = (
    <div className="flex items-center gap-2.5 border-t border-sidebar-border pt-3">
      <div className="flex size-[30px] shrink-0 items-center justify-center rounded-full bg-primary font-heading text-xs font-bold text-primary-foreground">
        {initialsFrom(me.data?.display)}
      </div>
      <div className="min-w-0 flex-1">
        <div className="truncate text-[13px] font-semibold text-sidebar-foreground">
          {me.data?.display}
        </div>
      </div>
      <Button variant="ghost" size="icon" className="max-md:min-h-11 max-md:min-w-11" aria-label={t("shell.logout")} onClick={handleLogout}>
        <LogOut className="size-4" aria-hidden="true" />
      </Button>
    </div>
  );

  // Theme toggle: lives in the topbar at ≥ md, moves to the drawer's footer
  // (alongside LanguageSwitcher) below md — same button, reused in both spots.
  const themeToggle = (
    <Button
      variant="outline"
      size="icon"
      className="size-[34px] max-md:min-h-11 max-md:min-w-11"
      aria-label={isDark ? t("shell.themeToLight") : t("shell.themeToDark")}
      onClick={toggle}
    >
      {isDark ? <Sun className="size-4" /> : <Moon className="size-4" />}
    </Button>
  );

  return (
    <>
      <div className="flex min-h-svh">
        <aside className="hidden shrink-0 flex-col border-r border-sidebar-border bg-sidebar px-3 py-4 md:flex md:w-[250px]">
          {sidebarHeader}

          <nav className="flex flex-col gap-4">
            {navGroups.map((group) => (
              <div key={group.label} className="flex flex-col gap-1">
                <div className="px-3 pb-2 font-mono text-[10px] font-medium tracking-[0.12em] text-sidebar-foreground/45 uppercase">
                  {group.label}
                </div>
                {group.items.map(({ to, label, icon: Icon }) => (
                  <NavLink
                    key={to}
                    to={to}
                    title={label}
                    className={({ isActive }) =>
                      cn(
                        "flex items-center gap-3 rounded-[9px] px-[11px] py-[9px] text-[14.5px] font-medium transition-colors",
                        isActive
                          ? "bg-sidebar-accent text-sidebar-accent-foreground"
                          : "text-sidebar-foreground/70 hover:bg-surface-hover",
                      )
                    }
                  >
                    <Icon className="size-[18px] shrink-0" aria-hidden="true" />
                    <span>{label}</span>
                  </NavLink>
                ))}
              </div>
            ))}
          </nav>

          <div className="flex-1" />

          {connectedLine}
          {userCard}
        </aside>
        <div className="flex min-w-0 flex-1 flex-col">
          <header className="flex h-[62px] shrink-0 items-center justify-between gap-3 border-b border-border px-4 md:px-6">
            <div className="flex flex-1 items-center gap-2.5">
              <Button
                variant="ghost"
                size="icon"
                className="size-11 md:hidden"
                aria-label={t("shell.openMenu")}
                onClick={() => setMobileNavOpen(true)}
              >
                <Menu className="size-5" aria-hidden="true" />
              </Button>

              <Button
                variant="ghost"
                size="icon"
                className="size-11 md:hidden"
                aria-label={searchExpanded ? t("shell.closeSearch") : t("shell.openSearch")}
                onClick={() => setSearchExpanded((expanded) => !expanded)}
              >
                <Search className="size-5" aria-hidden="true" />
              </Button>

              <div className="hidden max-w-[440px] flex-1 items-center gap-2 rounded-[10px] border border-border bg-secondary px-3.5 focus-within:border-ring focus-within:ring-3 focus-within:ring-ring/50 md:flex">
                <Search className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
                <input
                  type="text"
                  placeholder={t("shell.searchPlaceholder")}
                  aria-label={t("shell.searchPlaceholder")}
                  value={searchValue}
                  onChange={handleSearchChange}
                  onKeyDown={handleSearchKeyDown}
                  className="w-full min-w-0 flex-1 bg-transparent py-2 text-sm text-foreground outline-none placeholder:text-muted-foreground"
                />
              </div>
            </div>

            <div className="flex items-center gap-2.5">
              <div className="hidden items-center gap-2.5 md:flex">
                <LanguageSwitcher className="font-mono" />
                {themeToggle}
              </div>

              <Button
                onClick={() => navigate("/links?new=1")}
                aria-label={t("shell.newLink")}
                className="max-md:min-h-11 max-md:min-w-11"
              >
                <Plus className="size-4" aria-hidden="true" />
                <span className="hidden md:inline">{t("shell.newLink")}</span>
              </Button>
            </div>
          </header>
          {searchExpanded && (
            <div className="flex items-center gap-2 border-b border-border px-4 py-2.5 md:hidden">
              <div className="flex flex-1 items-center gap-2 rounded-[10px] border border-border bg-secondary px-3.5 focus-within:border-ring focus-within:ring-3 focus-within:ring-ring/50">
                <Search className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
                <input
                  type="text"
                  data-testid="mobile-search-input"
                  placeholder={t("shell.searchPlaceholder")}
                  aria-label={t("shell.searchPlaceholder")}
                  value={searchValue}
                  onChange={handleSearchChange}
                  onKeyDown={handleSearchKeyDown}
                  autoFocus
                  className="w-full min-w-0 flex-1 bg-transparent py-2 text-sm text-foreground outline-none placeholder:text-muted-foreground"
                />
              </div>
              <Button
                variant="ghost"
                size="icon"
                aria-label={t("shell.closeSearch")}
                onClick={() => setSearchExpanded(false)}
              >
                <XIcon className="size-4" aria-hidden="true" />
              </Button>
            </div>
          )}
          <main className="min-w-0 flex-1 overflow-auto p-6 sm:p-[26px_30px] sm:pb-[60px]">
            <Outlet />
          </main>
        </div>
      </div>

      <MobileNav
        open={mobileNavOpen}
        onOpenChange={setMobileNavOpen}
        groups={navGroups}
        footer={
          <>
            {connectedLine}
            {userCard}
            <div className="flex items-center justify-between gap-2 border-t border-sidebar-border pt-3">
              <LanguageSwitcher className="font-mono max-md:[&_button]:min-h-11 max-md:[&_button]:min-w-11" />
              {themeToggle}
            </div>
          </>
        }
      >
        {sidebarHeader}
      </MobileNav>
    </>
  );
}
