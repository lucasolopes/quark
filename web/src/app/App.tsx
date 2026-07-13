import { useEffect } from "react";
import { QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "react-router-dom";
import { Toaster } from "@/components/ui/sonner";
import { setUnauthorizedHandler } from "@/lib/api";
import { clearToken } from "@/lib/auth";
import { queryClient } from "@/lib/queries";
import { router } from "./router";
import { ThemeProvider } from "./theme";

export function App() {
  useEffect(() => {
    setUnauthorizedHandler(() => {
      clearToken();
      void router.navigate("/login");
    });
  }, []);

  return (
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
        <Toaster />
      </QueryClientProvider>
    </ThemeProvider>
  );
}

export default App;
