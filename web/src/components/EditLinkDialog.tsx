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
import { isHttpUrl } from "@/lib/codeguard";
import { isUnauthorized } from "@/lib/mutation-error";
import { usePatchLink } from "@/lib/queries";
import { formatTagsInput, parseTagsInput } from "@/lib/tags";
import { draftsFromRules, parseRuleDrafts, type RuleDraft } from "@/lib/rules";
import type { Link, Variant } from "@/lib/types";
import { RulesEditor } from "@/components/RulesEditor";

/** Same cap enforced server-side (`MAX_VARIANTS` in `src/api.rs`). */
const MAX_VARIANTS = 10;

interface VariantRow {
  url: string;
  weight: string;
}

function toVariantRows(variants: Variant[]): VariantRow[] {
  return variants.map((v) => ({ url: v.url, weight: String(v.weight) }));
}

function emptyVariantRow(): VariantRow {
  return { url: "", weight: "1" };
}

interface FormErrors {
  url?: string;
  ttl?: string;
  maxVisits?: string;
  rules?: string;
  form?: string;
  variants?: string;
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
  const [tagsInput, setTagsInput] = useState(formatTagsInput(link.tags ?? []));
  const [maxVisits, setMaxVisits] = useState(link.max_visits ? String(link.max_visits) : "");
  const [ruleDrafts, setRuleDrafts] = useState<RuleDraft[]>(() => draftsFromRules(link.rules));
  const [showVariants, setShowVariants] = useState(link.variants.length > 0);
  const [variantRows, setVariantRows] = useState<VariantRow[]>(() => toVariantRows(link.variants));
  const [errors, setErrors] = useState<FormErrors>({});
  const patchLink = usePatchLink();

  function addVariantRow() {
    setVariantRows((rows) => (rows.length >= MAX_VARIANTS ? rows : [...rows, emptyVariantRow()]));
  }

  function removeVariantRow(index: number) {
    setVariantRows((rows) => rows.filter((_, i) => i !== index));
  }

  function updateVariantRow(index: number, patch: Partial<VariantRow>) {
    setVariantRows((rows) => rows.map((row, i) => (i === index ? { ...row, ...patch } : row)));
  }

  function formatExpiry(expiry: number | null): string {
    if (expiry == null) return t("dialogs.edit.neverExpires");
    return t("dialogs.edit.expiresOn", { date: new Date(expiry * 1000).toLocaleDateString("pt-BR") });
  }

  function formatCurrentMaxVisits(value?: number): string {
    return value ? String(value) : t("dialogs.edit.unlimitedVisits");
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
    const trimmedMaxVisits = maxVisits.trim();
    if (trimmedMaxVisits) {
      const n = Number(trimmedMaxVisits);
      if (!Number.isInteger(n) || n <= 0) {
        next.maxVisits = t("dialogs.edit.maxVisitsInvalid");
      }
    }
    if (variantRows.length > MAX_VARIANTS) {
      next.variants = t("dialogs.edit.tooManyVariants", { max: MAX_VARIANTS });
    } else {
      for (const row of variantRows) {
        if (!row.url.trim() || !isHttpUrl(row.url)) {
          next.variants = t("dialogs.edit.variantUrlInvalid");
          break;
        }
        const w = Number(row.weight.trim());
        if (!Number.isInteger(w) || w <= 0) {
          next.variants = t("dialogs.edit.variantWeightInvalid");
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
          tags: parseTagsInput(tagsInput),
          ...(maxVisits.trim()
            ? { max_visits: Number(maxVisits.trim()) }
            : link.max_visits
              ? { max_visits: null }
              : {}),
          rules,
          variants: buildVariants(),
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

            <div className="flex flex-col gap-1.5">
              <label htmlFor="edit-link-tags" className="text-sm font-medium">
                {t("dialogs.edit.tagsLabel")} <span className="text-muted-foreground">({t("dialogs.edit.tagsHint")})</span>
              </label>
              <Input
                id="edit-link-tags"
                type="text"
                placeholder={t("dialogs.edit.tagsPlaceholder")}
                value={tagsInput}
                onChange={(e) => setTagsInput(e.target.value)}
              />
            </div>

            <div className="flex flex-col gap-1.5">
              <label htmlFor="edit-link-max-visits" className="text-sm font-medium">
                {t("dialogs.edit.maxVisitsLabel")} <span className="text-muted-foreground">{t("dialogs.edit.maxVisitsOptional")}</span>
              </label>
              <Input
                id="edit-link-max-visits"
                type="number"
                min={1}
                step={1}
                placeholder={t("dialogs.edit.maxVisitsPlaceholder", { current: formatCurrentMaxVisits(link.max_visits) })}
                value={maxVisits}
                onChange={(e) => setMaxVisits(e.target.value)}
                aria-invalid={errors.maxVisits != null}
              />
              {errors.maxVisits && (
                <p className="text-sm text-destructive" role="alert">
                  {errors.maxVisits}
                </p>
              )}
            </div>

            <RulesEditor idPrefix="edit-link" drafts={ruleDrafts} onChange={setRuleDrafts} />
            {errors.rules && (
              <p className="text-sm text-destructive" role="alert">
                {errors.rules}
              </p>
            )}

=======
            <div className="flex flex-col gap-2">
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="self-start"
                aria-expanded={showVariants}
                onClick={() => setShowVariants((v) => !v)}
              >
                {t("dialogs.edit.variantsToggle")}
              </Button>

              {showVariants && (
                <div className="flex flex-col gap-2 rounded-md border border-border p-3">
                  <p className="text-sm text-muted-foreground">{t("dialogs.edit.variantsHint")}</p>

                  {variantRows.map((row, i) => (
                    <div key={i} className="flex items-end gap-2">
                      <div className="flex flex-1 flex-col gap-1.5">
                        <label htmlFor={`edit-variant-url-${i}`} className="sr-only">
                          {t("dialogs.edit.variantUrlLabel")}
                        </label>
                        <Input
                          id={`edit-variant-url-${i}`}
                          type="text"
                          placeholder={t("dialogs.edit.variantUrlPlaceholder")}
                          value={row.url}
                          onChange={(e) => updateVariantRow(i, { url: e.target.value })}
                        />
                      </div>
                      <div className="flex w-20 flex-col gap-1.5">
                        <label htmlFor={`edit-variant-weight-${i}`} className="sr-only">
                          {t("dialogs.edit.variantWeightLabel")}
                        </label>
                        <Input
                          id={`edit-variant-weight-${i}`}
                          type="number"
                          min={1}
                          step={1}
                          placeholder={t("dialogs.edit.variantWeightLabel")}
                          value={row.weight}
                          onChange={(e) => updateVariantRow(i, { weight: e.target.value })}
                        />
                      </div>
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon-sm"
                        aria-label={t("dialogs.edit.removeVariant")}
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
                    {t("dialogs.edit.addVariant")}
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
            <Button type="submit" disabled={patchLink.isPending}>
              {patchLink.isPending ? t("dialogs.edit.submitting") : t("dialogs.edit.submit")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
