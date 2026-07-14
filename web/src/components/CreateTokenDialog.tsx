import { Check, Copy } from "lucide-react";
import { useState, type FormEvent } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { useT, type MessageKey } from "@/i18n";
import { isUnauthorized } from "@/lib/mutation-error";
import { useCreateToken } from "@/lib/queries";
import { ALL_SCOPES, type Scope } from "@/lib/types";

/** Message key (under `tokens.scope`) for each scope's display label. */
const SCOPE_LABEL_KEY: Record<Scope, MessageKey> = {
  links_read: "tokens.scope.linksRead",
  links_write: "tokens.scope.linksWrite",
  webhooks: "tokens.scope.webhooks",
  analytics: "tokens.scope.analytics",
  full: "tokens.scope.full",
};

interface FormErrors {
  name?: string;
  scopes?: string;
  rateLimit?: string;
  form?: string;
}

interface CreateTokenDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/**
 * Token creation dialog. Two phases: the form (name + scope checkboxes +
 * optional rate limit), then, on success, a one-time reveal of the plaintext
 * token with a copy button — the API never returns it again after this
 * response, so the dialog stays open until the operator explicitly
 * acknowledges (`Done`), rather than auto-closing like other create dialogs.
 */
export function CreateTokenDialog({ open, onOpenChange }: CreateTokenDialogProps) {
  const t = useT();
  const [name, setName] = useState("");
  const [scopes, setScopes] = useState<Scope[]>([]);
  const [rateLimit, setRateLimit] = useState("");
  const [errors, setErrors] = useState<FormErrors>({});
  const [createdToken, setCreatedToken] = useState<string | null>(null);
  const [justCopied, setJustCopied] = useState(false);
  const createToken = useCreateToken();

  function reset() {
    setName("");
    setScopes([]);
    setRateLimit("");
    setErrors({});
    setCreatedToken(null);
    setJustCopied(false);
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    onOpenChange(next);
  }

  function toggleScope(scope: Scope, checked: boolean) {
    setScopes((current) => (checked ? [...current, scope] : current.filter((s) => s !== scope)));
  }

  function validate(): FormErrors {
    const next: FormErrors = {};
    if (!name.trim()) next.name = t("tokens.nameRequired");
    if (scopes.length === 0) next.scopes = t("tokens.scopesRequired");
    const trimmedRate = rateLimit.trim();
    if (trimmedRate) {
      const n = Number(trimmedRate);
      if (!Number.isInteger(n) || n <= 0) next.rateLimit = t("tokens.rateLimitInvalid");
    }
    return next;
  }

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    const nextErrors = validate();
    if (Object.keys(nextErrors).length > 0) {
      setErrors(nextErrors);
      return;
    }
    setErrors({});
    try {
      const res = await createToken.mutateAsync({
        name: name.trim(),
        scopes,
        ...(rateLimit.trim() ? { rate_limit_per_min: Number(rateLimit.trim()) } : {}),
      });
      toast.success(t("tokens.createdSuccess"));
      setCreatedToken(res.token);
    } catch (err) {
      if (isUnauthorized(err)) return;
      setErrors({ form: t("tokens.createGenericError") });
    }
  }

  async function handleCopy() {
    if (!createdToken) return;
    try {
      await navigator.clipboard.writeText(createdToken);
      setJustCopied(true);
      toast.success(t("tokens.copied"));
      setTimeout(() => setJustCopied(false), 1500);
    } catch {
      toast.error(t("tokens.copyFailed"));
    }
  }

  if (createdToken) {
    return (
      <Dialog open={open} onOpenChange={handleOpenChange}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("tokens.createdTitle")}</DialogTitle>
            <DialogDescription>{t("tokens.createdWarning")}</DialogDescription>
          </DialogHeader>

          <div className="flex flex-col gap-1.5 py-3">
            <Label htmlFor="created-token-value">{t("tokens.tokenFieldLabel")}</Label>
            <div className="flex items-center gap-2">
              <Input id="created-token-value" type="text" readOnly value={createdToken} className="font-mono text-xs" />
              <Button type="button" variant="outline" onClick={handleCopy}>
                {justCopied ? <Check className="size-4 text-brand-ink" /> : <Copy className="size-4" />}
                {t("tokens.copyButton")}
              </Button>
            </div>
          </div>

          <DialogFooter>
            <Button type="button" onClick={() => handleOpenChange(false)}>
              {t("tokens.done")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    );
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>{t("tokens.createButton")}</DialogTitle>
            <DialogDescription>{t("tokens.subtitle")}</DialogDescription>
          </DialogHeader>

          <div className="flex flex-col gap-3 py-3">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="create-token-name">{t("tokens.nameLabel")}</Label>
              <Input
                id="create-token-name"
                type="text"
                placeholder={t("tokens.namePlaceholder")}
                value={name}
                onChange={(e) => setName(e.target.value)}
                aria-invalid={errors.name != null}
                autoFocus
              />
              {errors.name && (
                <p className="text-sm text-destructive" role="alert">
                  {errors.name}
                </p>
              )}
            </div>

            <div className="flex flex-col gap-1.5">
              <span className="text-sm font-medium">{t("tokens.scopesLabel")}</span>
              <div className="flex flex-col gap-2">
                {ALL_SCOPES.map((scope) => (
                  <Label key={scope} htmlFor={`scope-${scope}`} className="font-normal">
                    <Checkbox
                      id={`scope-${scope}`}
                      checked={scopes.includes(scope)}
                      onCheckedChange={(checked) => toggleScope(scope, checked === true)}
                    />
                    {t(SCOPE_LABEL_KEY[scope])}
                  </Label>
                ))}
              </div>
              {errors.scopes && (
                <p className="text-sm text-destructive" role="alert">
                  {errors.scopes}
                </p>
              )}
            </div>

            <div className="flex flex-col gap-1.5">
              <Label htmlFor="create-token-rate-limit">
                {t("tokens.rateLimitLabel")} <span className="text-muted-foreground">{t("tokens.rateLimitOptional")}</span>
              </Label>
              <Input
                id="create-token-rate-limit"
                type="number"
                min={1}
                step={1}
                placeholder={t("tokens.rateLimitPlaceholder")}
                value={rateLimit}
                onChange={(e) => setRateLimit(e.target.value)}
                aria-invalid={errors.rateLimit != null}
              />
              {errors.rateLimit && (
                <p className="text-sm text-destructive" role="alert">
                  {errors.rateLimit}
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
              {t("common.cancel")}
            </Button>
            <Button type="submit" disabled={createToken.isPending}>
              {createToken.isPending ? t("tokens.submitting") : t("tokens.submit")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
