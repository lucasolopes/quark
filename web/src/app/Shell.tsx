import { KeyRound, Link2, LogOut, Moon, Radio, Smartphone, Sun, Upload, Webhook } from "lucide-react";
import { useTheme } from "next-themes";
import { NavLink, Outlet, useNavigate } from "react-router-dom";
import { QuarkMark } from "@/components/brand/QuarkMark";
import { LanguageSwitcher } from "@/components/LanguageSwitcher";
import { Button } from "@/components/ui/button";
import { useT } from "@/i18n";
import { clearToken } from "@/lib/auth";
import { cn } from "@/lib/utils";

export function Shell() {
  const t = useT();
  const navigate = useNavigate();
  const { resolvedTheme, setTheme } = useTheme();
  const isDark = resolvedTheme === "dark";
  const toggle = () => setTheme(isDark ? "light" : "dark");

  const nav = [
    { to: "/links", label: t("shell.navLinks"), icon: Link2 },
    { to: "/webhooks", label: t("shell.navWebhooks"), icon: Webhook },
    { to: "/import", label: t("shell.navImport"), icon: Upload },
    { to: "/tokens", label: t("shell.navTokens"), icon: KeyRound },
    { to: "/pixels", label: t("shell.navPixels"), icon: Radio },
    { to: "/app-links", label: t("shell.navAppLinks"), icon: Smartphone },
  ];

  function handleLogout() {
    clearToken();
    navigate("/login", { replace: true });
  }

  return (
    <div className="flex min-h-svh">
      <aside className="flex w-16 shrink-0 flex-col border-r border-sidebar-border bg-sidebar sm:w-56">
        <div className="flex h-14 items-center justify-center gap-2.5 px-2 sm:justify-start sm:px-4">
          <QuarkMark className="size-6 text-primary drop-shadow-[0_0_8px_rgba(198,249,78,0.55)]" />
          <span className="hidden font-heading text-lg font-bold tracking-tight text-sidebar-foreground sm:inline">
            quark
          </span>
        </div>
        <nav className="flex flex-col gap-1 px-2 py-2">
          {nav.map(({ to, label, icon: Icon }) => (
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
        </nav>
      </aside>
      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex h-14 shrink-0 items-center justify-end gap-2 border-b border-border px-4">
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
