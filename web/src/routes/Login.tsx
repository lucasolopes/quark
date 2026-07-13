import { Atom, Loader2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useMutation } from "@tanstack/react-query";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { ApiError, api } from "@/lib/api";
import { clearToken, setToken } from "@/lib/auth";

export function Login() {
  const [value, setValue] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);
  const navigate = useNavigate();

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const mutation = useMutation({
    mutationFn: async (token: string) => {
      setToken(token);
      await api.listLinks({ limit: 1 });
    },
    onSuccess: () => {
      toast.success("Sessão iniciada.");
      navigate("/links", { replace: true });
    },
    onError: () => {
      clearToken();
    },
  });

  function handleSubmit(e: React.FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const token = value.trim();
    if (!token || mutation.isPending) return;
    mutation.mutate(token);
  }

  return (
    <div className="flex min-h-svh items-center justify-center bg-background p-4">
      <Card className="w-full max-w-sm">
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-xl">
            <Atom className="size-5 text-primary" aria-hidden="true" />
            quark
          </CardTitle>
          <CardDescription>Entre com o token de administrador para gerenciar os links.</CardDescription>
        </CardHeader>
        <CardContent>
          <form onSubmit={handleSubmit} className="flex flex-col gap-3" noValidate>
            <div className="flex flex-col gap-1.5">
              <label htmlFor="admin-token" className="text-sm font-medium">
                Token de admin
              </label>
              <Input
                id="admin-token"
                ref={inputRef}
                type="password"
                autoComplete="off"
                spellCheck={false}
                placeholder="••••••••"
                value={value}
                onChange={(e) => setValue(e.target.value)}
                aria-invalid={mutation.isError}
                aria-describedby={mutation.isError ? "admin-token-error" : undefined}
                className="font-mono"
              />
              {mutation.isError && (
                <p id="admin-token-error" role="alert" className="text-sm text-destructive">
                  {mutation.error instanceof ApiError && mutation.error.status === 401
                    ? "Token inválido. Verifique e tente novamente."
                    : "Não foi possível conectar. Tente novamente."}
                </p>
              )}
            </div>
            <Button type="submit" disabled={!value.trim() || mutation.isPending} className="mt-1">
              {mutation.isPending && <Loader2 className="size-4 animate-spin" aria-hidden="true" />}
              Entrar
            </Button>
          </form>
        </CardContent>
      </Card>
    </div>
  );
}
