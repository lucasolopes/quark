import { useT, type MessageKey } from "@/i18n";
import { DEFAULT_DURATION_UNIT } from "@/lib/duration";
import { cn } from "@/lib/utils";

interface TtlChipDef {
  value: string;
  unit: string;
  labelKey: MessageKey;
  /** Accessible name override for the "never" chip, whose visible label is just the ∞ glyph. */
  ariaKey?: MessageKey;
}

const CHIPS: TtlChipDef[] = [
  { value: "1", unit: "hours", labelKey: "dialogs.ttlChips.oneHour" },
  { value: "24", unit: "hours", labelKey: "dialogs.ttlChips.oneDay" },
  { value: "7", unit: "days", labelKey: "dialogs.ttlChips.sevenDays" },
  { value: "30", unit: "days", labelKey: "dialogs.ttlChips.thirtyDays" },
  { value: "", unit: DEFAULT_DURATION_UNIT, labelKey: "dialogs.ttlChips.never", ariaKey: "dialogs.ttlChips.neverAria" },
];

export interface TtlChipsProps {
  value: string;
  unit: string;
  onChange: (value: string, unit: string) => void;
  disabled?: boolean;
}

/**
 * Quick presets for link expiration (1h/24h/7d/30d/never) sitting above the
 * existing `DurationField` custom input. A chip just calls `onChange` with
 * the same `value`/`unit` pair the custom input's `onValueChange`/
 * `onUnitChange` would set — same state, same submit payload either way. The
 * custom input stays rendered below for anything the presets don't cover.
 *
 * Deliberately no group-level `aria-label`: the dialog's own "Expires in"
 * label already sits right below (on the custom input), and giving the
 * group that same accessible name would make `getByLabelText`-style queries
 * ambiguous between the two. Each chip's own text is its accessible name.
 */
export function TtlChips({ value, unit, onChange, disabled }: TtlChipsProps) {
  const t = useT();
  return (
    <div className="flex flex-wrap gap-1.5">
      {CHIPS.map((chip) => {
        const active = chip.value === "" ? value.trim() === "" : value.trim() === chip.value && unit === chip.unit;
        return (
          <button
            key={chip.labelKey}
            type="button"
            disabled={disabled}
            aria-pressed={active}
            aria-label={chip.ariaKey ? t(chip.ariaKey) : undefined}
            onClick={() => onChange(chip.value, chip.unit)}
            className={cn(
              "rounded-lg border px-3 py-1.5 text-sm font-medium transition-colors disabled:pointer-events-none disabled:opacity-50",
              active
                ? "bg-accent-wash border-accent-chip text-brand-ink"
                : "border-border text-muted-foreground hover:text-foreground",
            )}
          >
            {t(chip.labelKey)}
          </button>
        );
      })}
    </div>
  );
}
