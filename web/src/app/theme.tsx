import { ThemeProvider as NextThemesProvider } from "next-themes";
import type { ReactNode } from "react";

/**
 * Light/dark theme via the `.dark` class on `<html>`, persisted to
 * localStorage (next-themes' default behavior). The toggle itself uses
 * next-themes' `useTheme()` directly where it's consumed (Shell.tsx).
 */
export function ThemeProvider({ children }: { children: ReactNode }) {
  return (
    <NextThemesProvider attribute="class" defaultTheme="dark" enableSystem={false} disableTransitionOnChange>
      {children}
    </NextThemesProvider>
  );
}
