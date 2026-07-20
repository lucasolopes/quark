import { useState } from "react";
import type { useT } from "@/i18n";
import { isHttpUrl } from "@/lib/codeguard";
import type { Variant } from "@/lib/types";
import { distributeEvenly, variantsPercentTotal, MAX_VARIANTS } from "@/lib/variants";

export interface VariantRow {
  url: string;
  weight: string;
}

/** Reassign percentages so the rows split 100% evenly, keeping their URLs. */
function rebalance(rows: VariantRow[]): VariantRow[] {
  const pct = distributeEvenly(rows.length);
  return rows.map((row, i) => ({ ...row, weight: String(pct[i]) }));
}

/** i18n namespace each dialog owns; keeps `create-*` and `edit-*` strings separate. */
type VariantNamespace = "dialogs.create" | "dialogs.edit";

type TranslateFn = ReturnType<typeof useT>;

export interface UseVariantRows {
  rows: VariantRow[];
  addRow: () => void;
  removeRow: (index: number) => void;
  updateRow: (index: number, patch: Partial<VariantRow>) => void;
  distributeEvenly: () => void;
  total: number;
  totalValid: boolean;
  buildVariants: () => Variant[];
  /** Returns the variants error string (for `errors.variants`) or undefined. */
  validate: (t: TranslateFn, ns: VariantNamespace) => string | undefined;
  reset: () => void;
}

/**
 * A/B variant rows shared by the create and edit dialogs. Rows are edited as
 * percentage strings that must add up to 100; adding or removing a row (or the
 * explicit "distribute evenly" action) rebalances the split so it always sums
 * to 100 again.
 */
export function useVariantRows(initialRows: VariantRow[]): UseVariantRows {
  const [rows, setRows] = useState<VariantRow[]>(initialRows);

  function addRow() {
    setRows((rows) => (rows.length >= MAX_VARIANTS ? rows : rebalance([...rows, { url: "", weight: "0" }])));
  }

  function removeRow(index: number) {
    setRows((rows) => rebalance(rows.filter((_, i) => i !== index)));
  }

  function updateRow(index: number, patch: Partial<VariantRow>) {
    setRows((rows) => rows.map((row, i) => (i === index ? { ...row, ...patch } : row)));
  }

  function distribute() {
    setRows((rows) => rebalance(rows));
  }

  function reset() {
    setRows([]);
  }

  const total = variantsPercentTotal(rows.map((r) => r.weight));
  const totalValid = rows.length === 0 || total === 100;

  function buildVariants(): Variant[] {
    return rows.map((row) => ({ url: row.url.trim(), weight: Number(row.weight.trim()) }));
  }

  function validate(t: TranslateFn, ns: VariantNamespace): string | undefined {
    if (rows.length > MAX_VARIANTS) {
      return t(`${ns}.tooManyVariants`, { max: MAX_VARIANTS });
    }
    let sum = 0;
    for (const row of rows) {
      if (!row.url.trim() || !isHttpUrl(row.url)) {
        return t(`${ns}.variantUrlInvalid`);
      }
      const w = Number(row.weight.trim());
      if (!Number.isInteger(w) || w <= 0) {
        return t(`${ns}.variantPercentInvalid`);
      }
      sum += w;
    }
    if (rows.length > 0 && sum !== 100) {
      return t(`${ns}.variantsSumInvalid`, { total: sum });
    }
    return undefined;
  }

  return {
    rows,
    addRow,
    removeRow,
    updateRow,
    distributeEvenly: distribute,
    total,
    totalValid,
    buildVariants,
    validate,
    reset,
  };
}
