import { isHttpUrl } from "./codeguard";
import type { Rule, RuleField } from "./types";

/**
 * Editable draft of a `Rule` used by `CreateLinkDialog`/`EditLinkDialog` while
 * the user types. `valuesText` is the raw comma-separated input; it's only
 * split/trimmed into `Rule.values` on submit (see `parseRuleDrafts`), so the
 * user can type "BR, " without the trailing comma being rejected mid-typing.
 */
export interface RuleDraft {
  field: RuleField;
  valuesText: string;
  to: string;
}

export function emptyRuleDraft(): RuleDraft {
  return { field: "country", valuesText: "", to: "" };
}

/** Builds the editable drafts an `EditLinkDialog` pre-populates from a link's current rules. */
export function draftsFromRules(rules: Rule[]): RuleDraft[] {
  return rules.map((rule) => ({ field: rule.field, valuesText: rule.values.join(", "), to: rule.to }));
}

export type RuleParseError = "incomplete" | "invalidUrl";

/**
 * Converts rule drafts into the `Rule[]` payload the API expects. A row left
 * completely empty (no values, no destination) is dropped — it's just an
 * unfilled "add rule" row. A row with only one of values/destination filled
 * in, or a destination that isn't an http(s) URL, is a validation error: we
 * check the same http(s) prefix the main URL field already checks, to avoid
 * a round-trip for an error the backend would return anyway.
 */
export function parseRuleDrafts(drafts: RuleDraft[]): { rules: Rule[]; error?: RuleParseError } {
  const rules: Rule[] = [];
  for (const draft of drafts) {
    const values = draft.valuesText
      .split(",")
      .map((v) => v.trim())
      .filter((v) => v.length > 0);
    const to = draft.to.trim();
    if (values.length === 0 && !to) continue;
    if (values.length === 0 || !to) return { rules, error: "incomplete" };
    if (!isHttpUrl(to)) return { rules, error: "invalidUrl" };
    rules.push({ field: draft.field, values, to });
  }
  return { rules };
}
