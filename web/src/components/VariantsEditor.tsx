import { Plus, Trash2 } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useT } from "@/i18n";
import type { VariantRow } from "@/hooks/useVariantRows";
import { MAX_VARIANTS } from "@/lib/variants";

interface VariantsEditorProps {
  /** Prefix for input ids/labels, e.g. "create" or "edit" → `create-variant-url-0`. */
  idPrefix: string;
  /** i18n namespace owning the labels/errors: "dialogs.create" | "dialogs.edit". */
  ns: "dialogs.create" | "dialogs.edit";
  rows: VariantRow[];
  total: number;
  totalValid: boolean;
  error?: string;
  /** Whether the collapsible section starts expanded (edit opens it when the link has variants). */
  initialOpen: boolean;
  onAddRow: () => void;
  onRemoveRow: (index: number) => void;
  onUpdateRow: (index: number, patch: Partial<VariantRow>) => void;
  onDistributeEvenly: () => void;
}

/**
 * Collapsible A/B variants section shared by the create and edit dialogs. State
 * (the rows themselves, the running total, validation) lives in `useVariantRows`
 * in the parent; only the open/collapsed toggle is local here.
 */
export function VariantsEditor({
  idPrefix,
  ns,
  rows,
  total,
  totalValid,
  error,
  initialOpen,
  onAddRow,
  onRemoveRow,
  onUpdateRow,
  onDistributeEvenly,
}: VariantsEditorProps) {
  const t = useT();
  const [showVariants, setShowVariants] = useState(initialOpen);

  return (
    <div className="flex flex-col gap-2">
      <Button
        type="button"
        variant="outline"
        size="sm"
        className="self-start"
        aria-expanded={showVariants}
        onClick={() => setShowVariants((v) => !v)}
      >
        {t(`${ns}.variantsToggle`)}
      </Button>

      {showVariants && (
        <div className="flex flex-col gap-2 rounded-md border border-border p-3">
          <p className="text-sm text-muted-foreground">{t(`${ns}.variantsHint`)}</p>

          {rows.map((row, i) => (
            <div key={i} className="flex items-end gap-2">
              <div className="flex flex-1 flex-col gap-1.5">
                <label htmlFor={`${idPrefix}-variant-url-${i}`} className="sr-only">
                  {t(`${ns}.variantUrlLabel`)}
                </label>
                <Input
                  id={`${idPrefix}-variant-url-${i}`}
                  type="text"
                  placeholder={t(`${ns}.variantUrlPlaceholder`)}
                  value={row.url}
                  onChange={(e) => onUpdateRow(i, { url: e.target.value })}
                />
              </div>
              <div className="flex w-24 flex-col gap-1.5">
                <label htmlFor={`${idPrefix}-variant-weight-${i}`} className="sr-only">
                  {t(`${ns}.variantWeightLabel`)}
                </label>
                <div className="relative">
                  <Input
                    id={`${idPrefix}-variant-weight-${i}`}
                    type="number"
                    min={1}
                    max={100}
                    step={1}
                    className="pr-7"
                    value={row.weight}
                    onChange={(e) => onUpdateRow(i, { weight: e.target.value })}
                  />
                  <span className="pointer-events-none absolute inset-y-0 right-2.5 flex items-center text-sm text-muted-foreground">
                    %
                  </span>
                </div>
              </div>
              <Button
                type="button"
                variant="ghost"
                size="icon-sm"
                aria-label={t(`${ns}.removeVariant`)}
                onClick={() => onRemoveRow(i)}
              >
                <Trash2 className="size-3.5" />
              </Button>
            </div>
          ))}

          {rows.length > 0 && (
            <div className="flex items-center justify-between gap-2">
              <span
                className={
                  totalValid
                    ? "text-sm font-medium text-muted-foreground"
                    : "text-sm font-medium text-destructive"
                }
              >
                {t(`${ns}.variantsTotal`, { total })}
              </span>
              <Button type="button" variant="ghost" size="sm" onClick={onDistributeEvenly}>
                {t(`${ns}.distributeEvenly`)}
              </Button>
            </div>
          )}

          {error && (
            <p className="text-sm text-destructive" role="alert">
              {error}
            </p>
          )}

          <Button
            type="button"
            variant="outline"
            size="sm"
            className="self-start"
            disabled={rows.length >= MAX_VARIANTS}
            onClick={onAddRow}
          >
            <Plus className="size-3.5" />
            {t(`${ns}.addVariant`)}
          </Button>
        </div>
      )}
    </div>
  );
}
