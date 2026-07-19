import { Input } from "@/components/ui/input";
import { useT } from "@/i18n";
import { DURATION_UNITS } from "@/lib/duration";
import { cn } from "@/lib/utils";

interface DurationFieldProps {
  /** Base id; the value input uses it and the unit select uses `${id}-unit`. */
  id: string;
  label: string;
  /** Optional muted hint shown next to the label (e.g. "(optional)"). */
  hint?: string;
  value: string;
  unit: string;
  onValueChange: (value: string) => void;
  onUnitChange: (unit: string) => void;
  placeholder?: string;
  error?: string;
  disabled?: boolean;
}

/**
 * A whole-number value paired with a time-unit selector (minutes .. months).
 * Used for link expiration so the user never types a raw number of seconds.
 */
export function DurationField({
  id,
  label,
  hint,
  value,
  unit,
  onValueChange,
  onUnitChange,
  placeholder,
  error,
  disabled,
}: DurationFieldProps) {
  const t = useT();
  return (
    <div className="flex flex-col gap-1.5">
      <label htmlFor={id} className="text-sm font-medium">
        {label}
        {hint && <span className="text-muted-foreground"> {hint}</span>}
      </label>
      <div className="flex gap-2">
        <Input
          id={id}
          type="number"
          min={1}
          step={1}
          className="flex-1"
          placeholder={placeholder}
          value={value}
          onChange={(e) => onValueChange(e.target.value)}
          aria-invalid={error != null}
          disabled={disabled}
        />
        <label htmlFor={`${id}-unit`} className="sr-only">
          {t("dialogs.duration.unitLabel")}
        </label>
        <select
          id={`${id}-unit`}
          value={unit}
          onChange={(e) => onUnitChange(e.target.value)}
          disabled={disabled}
          className={cn(
            "h-8 w-32 shrink-0 rounded-lg border border-input bg-transparent px-2.5 py-1 text-sm outline-none transition-colors",
            "focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50",
            "disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50",
            "dark:bg-input/30",
          )}
        >
          {DURATION_UNITS.map((u) => (
            <option key={u.key} value={u.key}>
              {t(`dialogs.duration.units.${u.key}`)}
            </option>
          ))}
        </select>
      </div>
      {error && (
        <p className="text-sm text-destructive" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
