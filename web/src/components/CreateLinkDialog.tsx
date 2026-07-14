import { Plus, Trash2 } from "lucide-react";
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
import { isHttpUrl, isNumericCode } from "@/lib/codeguard";
import { isUnauthorized } from "@/lib/mutation-error";
import { useCreateLink } from "@/lib/queries";
import type { Variant } from "@/lib/types";

/** Same cap enforced server-side (`MAX_VARIANTS` in `src/api.rs`). */
const MAX_VARIANTS = 10;

interface VariantRow {
  url: string;
  weight: string;
}

function emptyVariantRow(): VariantRow {
  return { url: "", weight: "1" };
}

interface FormErrors {
  url?: string;
  alias?: string;
  ttl?: string;
  form?: string;
  variants?: string;
}

interface CreateLinkDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/**
 * Short link creation dialog. Validates client-side (http/https URL, alias
 * outside the numeric-code space, positive TTL) before calling the API —
 * avoids a round-trip just to get back an error we already knew about.
 */
export function CreateLinkDialog({ open, onOpenChange }: CreateLinkDialogProps) {
  const t = useT();
  const [url, setUrl] = useState("");
  const [alias, setAlias] = useState("");
  const [ttl, setTtl] = useState("");
  const [showVariants, setShowVariants] = useState(false);
  const [variantRows, setVariantRows] = useState<VariantRow[]>([]);
  const [errors, setErrors] = useState<FormErrors>({});
  const createLink = useCreateLink();

  function reset() {
    setUrl("");
    setAlias("");
    setTtl("");
    setShowVariants(false);
    setVariantRows([]);
    setErrors({});
  }

  function addVariantRow() {
    setVariantRows((rows) => (rows.length >= MAX_VARIANTS ? rows : [...rows, emptyVariantRow()]));
  }

  function removeVariantRow(index: number) {
    setVariantRows((rows) => rows.filter((_, i) => i !== index));
  }

  function updateVariantRow(index: number, patch: Partial<VariantRow>) {
    setVariantRows((rows) => rows.map((row, i) => (i === index ? { ...row, ...patch } : row)));
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    onOpenChange(next);
  }

  function validate(): FormErrors {
    const next: FormErrors = {};
    if (!url.trim()) {
      next.url = t("dialogs.create.urlRequired");
    } else if (!isHttpUrl(url)) {
      next.url = t("dialogs.create.urlInvalid");
    }
    const trimmedAlias = alias.trim();
    if (trimmedAlias && isNumericCode(trimmedAlias)) {
      next.alias = t("dialogs.create.aliasCollision");
    }
    const trimmedTtl = ttl.trim();
    if (trimmedTtl) {
      const n = Number(trimmedTtl);
      if (!Number.isInteger(n) || n <= 0) {
        next.ttl = t("dialogs.create.ttlInvalid");
      }
    }
    if (variantRows.length > MAX_VARIANTS) {
      next.variants = t("dialogs.create.tooManyVariants", { max: MAX_VARIANTS });
    } else {
      for (const row of variantRows) {
        if (!row.url.trim() || !isHttpUrl(row.url)) {
          next.variants = t("dialogs.create.variantUrlInvalid");
          break;
        }
        const w = Number(row.weight.trim());
        if (!Number.isInteger(w) || w <= 0) {
          next.variants = t("dialogs.create.variantWeightInvalid");
          break;
        }
      }
    }
    return next;
  }

  function buildVariants(): Variant[] {
    return variantRows.map((row) => ({ url: row.url.trim(), weight: Number(row.weight.trim()) }));
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
      const variants = buildVariants();
      await createLink.mutateAsync({
        url: url.trim(),
        ...(alias.trim() ? { alias: alias.trim() } : {}),
        ...(ttl.trim() ? { ttl: Number(ttl.trim()) } : {}),
        ...(variants.length > 0 ? { variants } : {}),
      });
      toast.success(t("dialogs.create.successToast"));
      reset();
      onOpenChange(false);
    } catch (err) {
      if (isUnauthorized(err)) return;
      if (err instanceof ApiError && err.status === 409) {
        setErrors({ alias: t("dialogs.create.aliasInUse") });
      } else if (err instanceof ApiError && err.status === 403) {
        setErrors({ url: t("dialogs.create.forbiddenDestination") });
      } else if (err instanceof ApiError && err.status === 429) {
        toast.error(t("common.rateLimited"));
      } else {
        setErrors({ form: t("dialogs.create.genericError") });
      }
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>{t("dialogs.create.title")}</DialogTitle>
            <DialogDescription>{t("dialogs.create.description")}</DialogDescription>
          </DialogHeader>

          <div className="flex flex-col gap-3 py-3">
            <div className="flex flex-col gap-1.5">
              <label htmlFor="create-link-url" className="text-sm font-medium">
                {t("dialogs.create.urlLabel")}
              </label>
              <Input
                id="create-link-url"
                type="text"
                placeholder={t("dialogs.create.urlPlaceholder")}
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
                {t("dialogs.create.aliasLabel")} <span className="text-muted-foreground">{t("dialogs.create.optional")}</span>
              </label>
              <Input
                id="create-link-alias"
                type="text"
                placeholder={t("dialogs.create.aliasPlaceholder")}
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
                {t("dialogs.create.ttlLabel")} <span className="text-muted-foreground">{t("dialogs.create.ttlOptional")}</span>
              </label>
              <Input
                id="create-link-ttl"
                type="number"
                min={1}
                step={1}
                placeholder={t("dialogs.create.ttlPlaceholder")}
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

            <div className="flex flex-col gap-2">
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="self-start"
                aria-expanded={showVariants}
                onClick={() => setShowVariants((v) => !v)}
              >
                {t("dialogs.create.variantsToggle")}
              </Button>

              {showVariants && (
                <div className="flex flex-col gap-2 rounded-md border border-border p-3">
                  <p className="text-sm text-muted-foreground">{t("dialogs.create.variantsHint")}</p>

                  {variantRows.map((row, i) => (
                    <div key={i} className="flex items-end gap-2">
                      <div className="flex flex-1 flex-col gap-1.5">
                        <label htmlFor={`create-variant-url-${i}`} className="sr-only">
                          {t("dialogs.create.variantUrlLabel")}
                        </label>
                        <Input
                          id={`create-variant-url-${i}`}
                          type="text"
                          placeholder={t("dialogs.create.variantUrlPlaceholder")}
                          value={row.url}
                          onChange={(e) => updateVariantRow(i, { url: e.target.value })}
                        />
                      </div>
                      <div className="flex w-20 flex-col gap-1.5">
                        <label htmlFor={`create-variant-weight-${i}`} className="sr-only">
                          {t("dialogs.create.variantWeightLabel")}
                        </label>
                        <Input
                          id={`create-variant-weight-${i}`}
                          type="number"
                          min={1}
                          step={1}
                          placeholder={t("dialogs.create.variantWeightLabel")}
                          value={row.weight}
                          onChange={(e) => updateVariantRow(i, { weight: e.target.value })}
                        />
                      </div>
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon-sm"
                        aria-label={t("dialogs.create.removeVariant")}
                        onClick={() => removeVariantRow(i)}
                      >
                        <Trash2 className="size-3.5" />
                      </Button>
                    </div>
                  ))}

                  {errors.variants && (
                    <p className="text-sm text-destructive" role="alert">
                      {errors.variants}
                    </p>
                  )}

                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    className="self-start"
                    disabled={variantRows.length >= MAX_VARIANTS}
                    onClick={addVariantRow}
                  >
                    <Plus className="size-3.5" />
                    {t("dialogs.create.addVariant")}
                  </Button>
                </div>
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
            <Button type="submit" disabled={createLink.isPending}>
              {createLink.isPending ? t("dialogs.create.submitting") : t("dialogs.create.submit")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
