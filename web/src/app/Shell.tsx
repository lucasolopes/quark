import { Link2, LogOut, Moon, ShieldBan, Sun } from "lucide-react";
import { useTheme } from "next-themes";
import { NavLink, Outlet, useNavigate } from "react-router-dom";
import { QuarkMark } from "@/components/brand/QuarkMark";
import { Button } from "@/components/ui/button";
import { clearToken } from "@/lib/auth";
import { cn } from "@/lib/utils";

const NAV = [
  { to: "/links", label: "Links", icon: Link2 },
  { to: "/blocklist", label: "Blocklist", icon: ShieldBan },
];

export function Shell() {
  const navigate = useNavigate();
  const { resolvedTheme, setTheme } = useTheme();
  const isDark = resolvedTheme === "dark";
  const toggle = () => setTheme(isDark ? "light" : "dark");

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
          {NAV.map(({ to, label, icon: Icon }) => (
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
          <Button
            variant="ghost"
            size="icon"
            aria-label={isDark ? "Usar tema claro" : "Usar tema escuro"}
            onClick={toggle}
          >
            {isDark ? <Sun className="size-4" /> : <Moon className="size-4" />}
          </Button>
          <Button variant="outline" size="sm" onClick={handleLogout}>
            <LogOut className="size-4" />
            Sair
          </Button>
        </header>
        <main className="min-w-0 flex-1 overflow-auto p-4 sm:p-6">
          <Outlet />
        </main>
      </div>
    </div>
  );
}
