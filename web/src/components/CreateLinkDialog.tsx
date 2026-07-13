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
import { isHttpUrl, isNumericCode } from "@/lib/codeguard";
import { useCreateLink } from "@/lib/queries";

interface FormErrors {
  url?: string;
  alias?: string;
  ttl?: string;
  form?: string;
}

interface CreateLinkDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

const GENERIC_ERROR = "Não foi possível criar o link. Tente de novo.";

/**
 * Dialog de criação de link curto. Valida no cliente (URL http/https, alias
 * fora do espaço de código numérico, TTL positivo) antes de chamar a API —
 * evita um round-trip só pra devolver um erro que já sabíamos de antemão.
 */
export function CreateLinkDialog({ open, onOpenChange }: CreateLinkDialogProps) {
  const [url, setUrl] = useState("");
  const [alias, setAlias] = useState("");
  const [ttl, setTtl] = useState("");
  const [errors, setErrors] = useState<FormErrors>({});
  const createLink = useCreateLink();

  function reset() {
    setUrl("");
    setAlias("");
    setTtl("");
    setErrors({});
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    onOpenChange(next);
  }

  function validate(): FormErrors {
    const next: FormErrors = {};
    if (!url.trim()) {
      next.url = "URL é obrigatória.";
    } else if (!isHttpUrl(url)) {
      next.url = "URL inválida — use um endereço http:// ou https://.";
    }
    const trimmedAlias = alias.trim();
    if (trimmedAlias && isNumericCode(trimmedAlias)) {
      next.alias = "Esse alias colide com um código gerado pelo sistema. Escolha outro.";
    }
    const trimmedTtl = ttl.trim();
    if (trimmedTtl) {
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
      await createLink.mutateAsync({
        url: url.trim(),
        ...(alias.trim() ? { alias: alias.trim() } : {}),
        ...(ttl.trim() ? { ttl: Number(ttl.trim()) } : {}),
      });
      toast.success("Link criado.");
      reset();
      onOpenChange(false);
    } catch (err) {
      if (err instanceof ApiError && err.status === 409) {
        setErrors({ alias: "Esse alias já está em uso." });
      } else if (err instanceof ApiError && err.status === 403) {
        setErrors({ url: "Esse destino não é permitido (pode estar bloqueado)." });
      } else if (err instanceof ApiError && err.status === 429) {
        toast.error("Muitas requisições. Tente de novo em um instante.");
      } else {
        setErrors({ form: GENERIC_ERROR });
      }
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Criar link</DialogTitle>
            <DialogDescription>Encurte uma URL e, se quiser, escolha um alias e um prazo de validade.</DialogDescription>
          </DialogHeader>

          <div className="flex flex-col gap-3 py-3">
            <div className="flex flex-col gap-1.5">
              <label htmlFor="create-link-url" className="text-sm font-medium">
                URL
              </label>
              <Input
                id="create-link-url"
                type="text"
                placeholder="https://exemplo.com/pagina"
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
              <label htmlFor="create-link-alias" className="text-sm font-medium">
                Alias <span className="text-muted-foreground">(opcional)</span>
              </label>
              <Input
                id="create-link-alias"
                type="text"
                placeholder="promo-verao"
                value={alias}
                onChange={(e) => setAlias(e.target.value)}
                aria-invalid={errors.alias != null}
              />
              {errors.alias && (
                <p className="text-sm text-destructive" role="alert">
                  {errors.alias}
                </p>
              )}
            </div>

            <div className="flex flex-col gap-1.5">
              <label htmlFor="create-link-ttl" className="text-sm font-medium">
                Expira em <span className="text-muted-foreground">(segundos, opcional)</span>
              </label>
              <Input
                id="create-link-ttl"
                type="number"
                min={1}
                step={1}
                placeholder="Sem prazo — nunca expira"
                value={ttl}
                onChange={(e) => setTtl(e.target.value)}
                aria-invalid={errors.ttl != null}
              />
              {errors.ttl && (
                <p className="text-sm text-destructive" role="alert">
                  {errors.ttl}
                </p>
              )}
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
            <Button type="submit" disabled={createLink.isPending}>
              {createLink.isPending ? "Criando…" : "Criar link"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
