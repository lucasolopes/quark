import { useMemo } from "react";
import { Plus, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Combobox, type ComboboxOption } from "@/components/Combobox";
import { useT, useLocale } from "@/i18n";
import { emptyRuleDraft, type RuleDraft } from "@/lib/rules";
import { countryOptions } from "@/lib/countries";
import type { RuleField } from "@/lib/types";

/** Split the stored comma-separated values into an array of trimmed, non-empty entries. */
function parseValues(text: string): string[] {
  return text
    .split(",")
    .map((v) => v.trim())
    .filter((v) => v.length > 0);
}

interface RulesEditorProps {
  idPrefix: string;
  drafts: RuleDraft[];
  onChange: (drafts: RuleDraft[]) => void;
}

/**
 * Optional, collapsible "Redirect rules" section shared by CreateLinkDialog
 * and EditLinkDialog (roadmap #12). Each row edits one `RuleDraft`: a field
 * (country/device), a comma-separated values input, and a destination URL.
 * The main `url` field on the dialog stays the default destination — this
 * component never touches it, it only manages the rule list.
 *
 * Starts expanded when there are drafts already (EditLinkDialog on a link
 * that has rules), collapsed otherwise — a plain `<details>` keeps this
 * accessible without extra state plumbing from the parent.
 */
export function RulesEditor({ idPrefix, drafts, onChange }: RulesEditorProps) {
  const t = useT();
  const { locale } = useLocale();
  const countryOpts = useMemo(() => countryOptions(locale), [locale]);
  const deviceOpts: ComboboxOption[] = useMemo(
    () => [
      { value: "Mobile", label: t("rules.deviceMobile") },
      { value: "Desktop", label: t("rules.deviceDesktop") },
      { value: "Other", label: t("rules.deviceOther") },
    ],
    [t],
  );

  function updateRow(index: number, patch: Partial<RuleDraft>) {
    onChange(drafts.map((draft, i) => (i === index ? { ...draft, ...patch } : draft)));
  }

  function removeRow(index: number) {
    onChange(drafts.filter((_, i) => i !== index));
  }

  function addRow() {
    onChange([...drafts, emptyRuleDraft()]);
  }

  return (
    <details className="rounded-lg border border-input px-3 py-2" open={drafts.length > 0}>
      <summary className="cursor-pointer text-sm font-medium">
        {t("rules.sectionTitle")}
        {drafts.length > 0 && <span className="text-muted-foreground"> ({drafts.length})</span>}
      </summary>
      <p className="mt-2 text-sm text-muted-foreground">{t("rules.sectionDescription")}</p>

      <div className="mt-3 flex flex-col gap-3">
        {drafts.map((draft, index) => {
          const rowId = `${idPrefix}-rule-${index}`;
          return (
            <div key={index} className="flex flex-col gap-2 rounded-md border border-border p-2 sm:flex-row sm:items-end">
              <div className="flex flex-col gap-1.5">
                <label htmlFor={`${rowId}-field`} className="text-xs font-medium text-muted-foreground">
                  {t("rules.fieldLabel")}
                </label>
                <select
                  id={`${rowId}-field`}
                  className="h-8 rounded-lg border border-input bg-transparent px-2.5 text-sm outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50"
                  value={draft.field}
                  onChange={(e) => updateRow(index, { field: e.target.value as RuleField, valuesText: "" })}
                >
                  <option value="country">{t("rules.fieldCountry")}</option>
                  <option value="device">{t("rules.fieldDevice")}</option>
                </select>
              </div>

              <div className="flex flex-1 flex-col gap-1.5">
                <label htmlFor={`${rowId}-values`} className="text-xs font-medium text-muted-foreground">
                  {t("rules.valuesLabel")}
                </label>
                <Combobox
                  id={`${rowId}-values`}
                  multiple
                  options={draft.field === "device" ? deviceOpts : countryOpts}
                  value={parseValues(draft.valuesText)}
                  onChange={(vals) => updateRow(index, { valuesText: vals.join(",") })}
                  ariaLabel={t("rules.valuesLabel")}
                  placeholder={draft.field === "device" ? t("rules.valuesPlaceholderDevice") : t("rules.valuesPlaceholderCountry")}
                />
              </div>

              <div className="flex flex-1 flex-col gap-1.5">
                <label htmlFor={`${rowId}-to`} className="text-xs font-medium text-muted-foreground">
                  {t("rules.destinationLabel")}
                </label>
                <Input
                  id={`${rowId}-to`}
                  type="text"
                  placeholder={t("rules.destinationPlaceholder")}
                  value={draft.to}
                  onChange={(e) => updateRow(index, { to: e.target.value })}
                />
              </div>

              <Button
                type="button"
                variant="ghost"
                size="icon-sm"
                aria-label={t("rules.removeRuleAria", { index: index + 1 })}
                onClick={() => removeRow(index)}
              >
                <Trash2 className="size-3.5" />
              </Button>
            </div>
          );
        })}

        <Button type="button" variant="outline" size="sm" onClick={addRow} className="self-start">
          <Plus className="size-3.5" />
          {t("rules.addRule")}
        </Button>
      </div>
    </details>
  );
}
