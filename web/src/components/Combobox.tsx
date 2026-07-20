import { useId, useMemo, useRef, useState } from "react";
import { Check, Plus, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useT } from "@/i18n";
import { cn } from "@/lib/utils";

export interface ComboboxOption {
  value: string;
  label: string;
}

interface ComboboxProps {
  id?: string;
  options: ComboboxOption[];
  /** Selected values. Single-select holds at most one. */
  value: string[];
  onChange: (value: string[]) => void;
  /** Allow selecting more than one option (chips + checkboxes). Default false. */
  multiple?: boolean;
  /**
   * When set, a "create" button with this label appears below the field. It
   * reveals a name input so the user creates a value explicitly, instead of the
   * value being created inline while searching. Omit for select-only fields.
   */
  createLabel?: string;
  placeholder?: string;
  ariaLabel?: string;
  disabled?: boolean;
  invalid?: boolean;
}

/**
 * A searchable select with removable chips. Used for tags (multi, creatable),
 * folders (single, creatable), and geo/device rule values (select-only). The
 * user picks from existing options instead of retyping, which avoids typos and
 * makes values reusable. Multi-select options carry a checkbox and toggle
 * without closing the list.
 */
export function Combobox({
  id,
  options,
  value,
  onChange,
  multiple = false,
  createLabel,
  placeholder,
  ariaLabel,
  disabled,
  invalid,
}: ComboboxProps) {
  const t = useT();
  const generatedId = useId();
  const inputId = id ?? generatedId;
  const listboxId = `${inputId}-listbox`;
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(0);
  const [creating, setCreating] = useState(false);
  const [createDraft, setCreateDraft] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  const labelFor = (val: string) => options.find((o) => o.value === val)?.label ?? val;

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return options.filter((o) => {
      // Single-select hides the already-picked option; multi keeps it (checked).
      if (!multiple && value.includes(o.value)) return false;
      return q === "" || o.label.toLowerCase().includes(q) || o.value.toLowerCase().includes(q);
    });
  }, [options, value, query, multiple]);

  function toggleValue(val: string) {
    if (multiple) {
      onChange(value.includes(val) ? value.filter((v) => v !== val) : [...value, val]);
      inputRef.current?.focus(); // keep the list open for more picks
    } else {
      onChange([val]);
      setQuery("");
      setOpen(false);
    }
  }

  function removeValue(val: string) {
    onChange(value.filter((v) => v !== val));
  }

  function confirmCreate() {
    const name = createDraft.trim();
    if (name) {
      if (multiple) {
        if (!value.includes(name)) onChange([...value, name]);
      } else {
        onChange([name]);
      }
    }
    setCreateDraft("");
    setCreating(false);
  }

  function cancelCreate() {
    setCreateDraft("");
    setCreating(false);
  }

  function handleKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setOpen(true);
      setActiveIndex((i) => (filtered.length === 0 ? 0 : Math.min(i + 1, filtered.length - 1)));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActiveIndex((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter") {
      if (open && filtered.length > 0) {
        e.preventDefault();
        toggleValue(filtered[Math.min(activeIndex, filtered.length - 1)].value);
      }
    } else if (e.key === "Backspace" && query === "" && value.length > 0) {
      removeValue(value[value.length - 1]);
    } else if (e.key === "Escape") {
      setOpen(false);
    }
  }

  return (
    <div>
      <div className="relative">
        <div
          className={cn(
            "flex min-h-8 w-full flex-wrap items-center gap-1.5 rounded-lg border border-input bg-transparent px-2 py-1 text-sm transition-colors",
            "focus-within:border-ring focus-within:ring-3 focus-within:ring-ring/50",
            "dark:bg-input/30",
            disabled && "pointer-events-none opacity-50",
            invalid && "border-destructive ring-3 ring-destructive/20 dark:ring-destructive/40",
          )}
          onMouseDown={(e) => {
            if (e.target === e.currentTarget) inputRef.current?.focus();
          }}
        >
          {value.map((val) => (
            <span
              key={val}
              className="inline-flex items-center gap-1 rounded-md bg-secondary px-1.5 py-0.5 text-xs font-medium text-secondary-foreground"
            >
              {labelFor(val)}
              <button
                type="button"
                className="text-secondary-foreground/70 hover:text-secondary-foreground"
                aria-label={t("combobox.removeValue", { value: labelFor(val) })}
                onClick={() => removeValue(val)}
              >
                <X className="size-3" />
              </button>
            </span>
          ))}
          <input
            ref={inputRef}
            id={inputId}
            type="text"
            className="min-w-16 flex-1 bg-transparent outline-none placeholder:text-muted-foreground"
            value={query}
            placeholder={value.length === 0 ? placeholder : ""}
            aria-label={ariaLabel}
            aria-expanded={open}
            aria-controls={open ? listboxId : undefined}
            aria-autocomplete="list"
            aria-activedescendant={open && filtered.length > 0 ? `${listboxId}-opt-${Math.min(activeIndex, filtered.length - 1)}` : undefined}
            role="combobox"
            autoComplete="off"
            disabled={disabled}
            onChange={(e) => {
              setQuery(e.target.value);
              setOpen(true);
              setActiveIndex(0);
            }}
            onFocus={() => setOpen(true)}
            onBlur={() => setOpen(false)}
            onKeyDown={handleKeyDown}
          />
        </div>

        {open && filtered.length > 0 && (
          <ul
            id={listboxId}
            role="listbox"
            aria-multiselectable={multiple || undefined}
            className="absolute z-50 mt-1 max-h-56 w-full overflow-y-auto rounded-lg border border-border bg-popover p-1 text-sm shadow-md"
          >
            {filtered.map((o, i) => {
              const selected = value.includes(o.value);
              return (
                <li
                  key={o.value}
                  id={`${listboxId}-opt-${i}`}
                  role="option"
                  aria-selected={selected}
                  className={cn(
                    "flex cursor-pointer items-center gap-2 rounded-md px-2 py-1.5",
                    i === activeIndex ? "bg-muted" : "hover:bg-muted",
                  )}
                  // mousedown keeps input focus so the list does not blur-close first
                  onMouseDown={(e) => {
                    e.preventDefault();
                    toggleValue(o.value);
                  }}
                  onMouseEnter={() => setActiveIndex(i)}
                >
                  {multiple && (
                    <span
                      aria-hidden
                      className={cn(
                        "flex size-4 shrink-0 items-center justify-center rounded border",
                        selected ? "border-primary bg-primary text-primary-foreground" : "border-input",
                      )}
                    >
                      {selected && <Check className="size-3" />}
                    </span>
                  )}
                  {o.label}
                </li>
              );
            })}
          </ul>
        )}
      </div>

      {createLabel && !creating && (
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="mt-1.5 self-start px-1.5"
          disabled={disabled}
          onClick={() => setCreating(true)}
        >
          <Plus className="size-3.5" />
          {createLabel}
        </Button>
      )}

      {createLabel && creating && (
        <div className="mt-1.5 flex items-center gap-2">
          <Input
            autoFocus
            value={createDraft}
            placeholder={t("combobox.createPlaceholder")}
            aria-label={createLabel}
            onChange={(e) => setCreateDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                confirmCreate();
              } else if (e.key === "Escape") {
                cancelCreate();
              }
            }}
          />
          <Button type="button" size="sm" onClick={confirmCreate} disabled={createDraft.trim() === ""}>
            {t("combobox.add")}
          </Button>
          <Button type="button" variant="ghost" size="sm" onClick={cancelCreate}>
            {t("combobox.cancel")}
          </Button>
        </div>
      )}
    </div>
  );
}
