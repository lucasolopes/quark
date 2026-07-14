import { ThemeProvider as NextThemesProvider } from "next-themes";
import type { ReactNode } from "react";

/**
 * Tema claro/escuro via classe `.dark` no `<html>`, persistido em
 * localStorage (comportamento padrão do next-themes). O toggle em si usa
 * `useTheme()` do next-themes diretamente onde é consumido (Shell.tsx).
 */
export function ThemeProvider({ children }: { children: ReactNode }) {
  return (
    <NextThemesProvider attribute="class" defaultTheme="dark" enableSystem={false} disableTransitionOnChange>
      {children}
    </NextThemesProvider>
  );
}
