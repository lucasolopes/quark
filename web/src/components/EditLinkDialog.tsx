import { useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { ApiError } from "@/lib/api";
import { isHttpUrl } from "@/lib/codeguard";
import { isUnauthorized } from "@/lib/mutation-error";
import { usePatchLink } from "@/lib/queries";
import type { Link } from "@/lib/types";

interface FormErrors {
  url?: string;
  ttl?: string;
  form?: string;
}

interface EditLinkDialogProps {
  link: Link;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

const GENERIC_ERROR = "Não foi possível salvar as alterações. Tente de novo.";

function formatExpiry(expiry: number | null): string {
  if (expiry == null) return "nunca expira";
  return `expira em ${new Date(expiry * 1000).toLocaleDateString("pt-BR")}`;
}

/**
 * Dialog de edição de um link existente. Monta com `key={link.code}` no
 * chamador (Links.tsx) para que os campos sempre partam do link certo —
 * sem isso precisaríamos sincronizar estado via efeito a cada troca de link.
 */
export function EditLinkDialog({ link, open, onOpenChange }: EditLinkDialogProps) {
  const [url, setUrl] = useState(link.url);
  const [ttl, setTtl] = useState("");
  const [removeExpiry, setRemoveExpiry] = useState(false);
  const [errors, setErrors] = useState<FormErrors>({});
  const patchLink = usePatchLink();

  function handleOpenChange(next: boolean) {
    if (!next) setErrors({});
    onOpenChange(next);
  }

  function validate(): FormErrors {
    const next: FormErrors = {};
    if (!url.trim()) {
      next.url = "URL é obrigatória.";
    } else if (!isHttpUrl(url)) {
      next.url = "URL inválida — use um endereço http:// ou https://.";
    }
    const trimmedTtl = ttl.trim();
    if (!removeExpiry && trimmedTtl) {
      const n = Number(trimmedTtl);
      if (!Number.isInteger(n) || n <= 0) {
        next.ttl = "TTL deve ser um número de segundos maior que zero.";
      }
    }
    return next;
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const nextErrors = validate();
    if (Object.keys(nextErrors).length > 0) {
      setErrors(nextErrors);
      return;
    }
    setErrors({});
    try {
      await patchLink.mutateAsync({
        code: link.code,
        body: {
          url: url.trim(),
          // `ttl: null` remove a expiração (o backend aceita isso no PATCH).
          // Sem marcar "sem expiração", só manda `ttl` se o campo foi preenchido.
          ...(removeExpiry ? { ttl: null } : ttl.trim() ? { ttl: Number(ttl.trim()) } : {}),
        },
      });
      toast.success("Link atualizado.");
      onOpenChange(false);
    } catch (err) {
      // 401: o handler global já limpa o token e redireciona pro /login —
      // não duplica feedback aqui.
      if (isUnauthorized(err)) return;
      if (err instanceof ApiError && err.status === 403) {
        setErrors({ url: "Esse destino não é permitido (pode estar bloqueado)." });
      } else if (err instanceof ApiError && err.status === 429) {
        toast.error("Muitas requisições. Tente de novo em um instante.");
      } else {
        // O PATCH nunca devolve 409 (sem alias na edição, não há colisão a
        // detectar) — sem branch dedicado pra esse status.
        setErrors({ form: GENERIC_ERROR });
      }
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Editar {link.code}</DialogTitle>
            <DialogDescription>Atualize o destino e/ou o prazo de validade deste link.</DialogDescription>
          </DialogHeader>

          <div className="flex flex-col gap-3 py-3">
            <div className="flex flex-col gap-1.5">
              <label htmlFor="edit-link-url" className="text-sm font-medium">
                URL
              </label>
              <Input
                id="edit-link-url"
                type="text"
                value={url}
                onChange={(e) => setUrl(e.target.value)}
                aria-invalid={errors.url != null}
                autoFocus
              />
              {errors.url && (
                <p className="text-sm text-destructive" role="alert">
                  {errors.url}
                </p>
              )}
            </div>

            <div className="flex flex-col gap-1.5">
              <label htmlFor="edit-link-ttl" className="text-sm font-medium">
                Novo prazo <span className="text-muted-foreground">(segundos a partir de agora, opcional)</span>
              </label>
              <Input
                id="edit-link-ttl"
                type="number"
                min={1}
                step={1}
                placeholder={`Atualmente ${formatExpiry(link.expiry)}`}
                value={ttl}
                onChange={(e) => setTtl(e.target.value)}
                aria-invalid={errors.ttl != null}
                disabled={removeExpiry}
              />
              {errors.ttl && (
                <p className="text-sm text-destructive" role="alert">
                  {errors.ttl}
                </p>
              )}
              <label className="flex items-center gap-2 text-sm text-muted-foreground">
                <input
                  type="checkbox"
                  className="size-4 rounded border-input accent-primary"
                  checked={removeExpiry}
                  onChange={(e) => {
                    setRemoveExpiry(e.target.checked);
                    if (e.target.checked) setTtl("");
                  }}
                />
                Remover expiração (link nunca expira)
              </label>
            </div>

            {errors.form && (
              <p className="text-sm text-destructive" role="alert">
                {errors.form}
              </p>
            )}
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => handleOpenChange(false)}>
              Cancelar
            </Button>
            <Button type="submit" disabled={patchLink.isPending}>
              {patchLink.isPending ? "Salvando…" : "Salvar alterações"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
