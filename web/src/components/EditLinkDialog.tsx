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
import { useT } from "@/i18n";
import { ApiError } from "@/lib/api";
import { isHttpUrl } from "@/lib/codeguard";
import { isUnauthorized } from "@/lib/mutation-error";
import { usePatchLink } from "@/lib/queries";
import { draftsFromRules, parseRuleDrafts, type RuleDraft } from "@/lib/rules";
import type { Link } from "@/lib/types";
import { RulesEditor } from "@/components/RulesEditor";

interface FormErrors {
  url?: string;
  ttl?: string;
  rules?: string;
  form?: string;
}

interface EditLinkDialogProps {
  link: Link;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/**
 * Dialog for editing an existing link. Mounted with `key={link.code}` by the
 * caller (Links.tsx) so the fields always start from the right link — without
 * that we'd need to sync state via an effect on every link change.
 */
export function EditLinkDialog({ link, open, onOpenChange }: EditLinkDialogProps) {
  const t = useT();
  const [url, setUrl] = useState(link.url);
  const [ttl, setTtl] = useState("");
  const [removeExpiry, setRemoveExpiry] = useState(false);
  const [ruleDrafts, setRuleDrafts] = useState<RuleDraft[]>(() => draftsFromRules(link.rules));
  const [errors, setErrors] = useState<FormErrors>({});
  const patchLink = usePatchLink();

  function formatExpiry(expiry: number | null): string {
    if (expiry == null) return t("dialogs.edit.neverExpires");
    return t("dialogs.edit.expiresOn", { date: new Date(expiry * 1000).toLocaleDateString("pt-BR") });
  }

  function handleOpenChange(next: boolean) {
    if (!next) setErrors({});
    onOpenChange(next);
  }

  function validate(): FormErrors {
    const next: FormErrors = {};
    if (!url.trim()) {
      next.url = t("dialogs.edit.urlRequired");
    } else if (!isHttpUrl(url)) {
      next.url = t("dialogs.edit.urlInvalid");
    }
    const trimmedTtl = ttl.trim();
    if (!removeExpiry && trimmedTtl) {
      const n = Number(trimmedTtl);
      if (!Number.isInteger(n) || n <= 0) {
        next.ttl = t("dialogs.edit.ttlInvalid");
      }
    }
    return next;
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const nextErrors = validate();
    const { rules, error: rulesError } = parseRuleDrafts(ruleDrafts);
    if (rulesError) {
      nextErrors.rules = t(rulesError === "invalidUrl" ? "rules.rowInvalidUrl" : "rules.rowIncomplete");
    }
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
          ...(removeExpiry ? { ttl: null } : ttl.trim() ? { ttl: Number(ttl.trim()) } : {}),
          rules,
        },
      });
      toast.success(t("dialogs.edit.successToast"));
      onOpenChange(false);
    } catch (err) {
      if (isUnauthorized(err)) return;
      if (err instanceof ApiError && err.status === 403) {
        setErrors({ url: t("dialogs.edit.forbiddenDestination") });
      } else if (err instanceof ApiError && err.status === 429) {
        toast.error(t("common.rateLimited"));
      } else {
        setErrors({ form: t("dialogs.edit.genericError") });
      }
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>{t("dialogs.edit.title", { code: link.code })}</DialogTitle>
            <DialogDescription>{t("dialogs.edit.description")}</DialogDescription>
          </DialogHeader>

          <div className="flex flex-col gap-3 py-3">
            <div className="flex flex-col gap-1.5">
              <label htmlFor="edit-link-url" className="text-sm font-medium">
                {t("dialogs.edit.urlLabel")}
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
                {t("dialogs.edit.ttlLabel")} <span className="text-muted-foreground">{t("dialogs.edit.ttlOptional")}</span>
              </label>
              <Input
                id="edit-link-ttl"
                type="number"
                min={1}
                step={1}
                placeholder={t("dialogs.edit.ttlPlaceholder", { expiry: formatExpiry(link.expiry) })}
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
                {t("dialogs.edit.removeExpiryLabel")}
              </label>
            </div>

            <RulesEditor idPrefix="edit-link" drafts={ruleDrafts} onChange={setRuleDrafts} />
            {errors.rules && (
              <p className="text-sm text-destructive" role="alert">
                {errors.rules}
              </p>
            )}

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
            <Button type="submit" disabled={patchLink.isPending}>
              {patchLink.isPending ? t("dialogs.edit.submitting") : t("dialogs.edit.submit")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
