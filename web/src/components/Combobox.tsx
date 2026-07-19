import { useId, useMemo, useRef, useState } from "react";
import { X } from "lucide-react";
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
  /** Allow selecting more than one option (chips). Default false. */
  multiple?: boolean;
  /** Allow adding a value that is not in `options` (typed and created). Default false. */
  creatable?: boolean;
  placeholder?: string;
  ariaLabel?: string;
  disabled?: boolean;
  invalid?: boolean;
}

/**
 * A searchable select with removable chips. Used for tags (multi, creatable),
 * folders (single, creatable), and geo/device rule values (select-only). The
 * user picks from existing options instead of retyping, which avoids typos and
 * makes values reusable.
 */
export function Combobox({
  id,
  options,
  value,
  onChange,
  multiple = false,
  creatable = false,
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
  const inputRef = useRef<HTMLInputElement>(null);

  const labelFor = (val: string) => options.find((o) => o.value === val)?.label ?? val;

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return options.filter(
      (o) => !value.includes(o.value) && (q === "" || o.label.toLowerCase().includes(q) || o.value.toLowerCase().includes(q)),
    );
  }, [options, value, query]);

  const trimmed = query.trim();
  const showCreate =
    creatable &&
    trimmed !== "" &&
    !value.some((v) => v.toLowerCase() === trimmed.toLowerCase()) &&
    !options.some((o) => o.value.toLowerCase() === trimmed.toLowerCase() || o.label.toLowerCase() === trimmed.toLowerCase());

  // The navigable rows: filtered options followed by the optional "create" row.
  const rowCount = filtered.length + (showCreate ? 1 : 0);

  function commit(next: string[]) {
    onChange(next);
    setQuery("");
    setActiveIndex(0);
    if (multiple) {
      inputRef.current?.focus();
    } else {
      setOpen(false);
    }
  }

  function selectValue(val: string) {
    if (value.includes(val)) return;
    commit(multiple ? [...value, val] : [val]);
  }

  function removeValue(val: string) {
    onChange(value.filter((v) => v !== val));
  }

  function activate(index: number) {
    if (index < filtered.length) {
      selectValue(filtered[index].value);
    } else if (showCreate) {
      selectValue(trimmed);
    }
  }

  function handleKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setOpen(true);
      setActiveIndex((i) => (rowCount === 0 ? 0 : Math.min(i + 1, rowCount - 1)));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActiveIndex((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter") {
      if (open && rowCount > 0) {
        e.preventDefault();
        activate(Math.min(activeIndex, rowCount - 1));
      }
    } else if (e.key === "Backspace" && query === "" && value.length > 0) {
      removeValue(value[value.length - 1]);
    } else if (e.key === "Escape") {
      setOpen(false);
    }
  }

  return (
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
          // Clicks on the empty area focus the input without stealing focus from chips.
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

      {open && rowCount > 0 && (
        <ul
          id={listboxId}
          role="listbox"
          className="absolute z-50 mt-1 max-h-56 w-full overflow-y-auto rounded-lg border border-border bg-popover p-1 text-sm shadow-md"
        >
          {filtered.map((o, i) => (
            <li
              key={o.value}
              role="option"
              aria-selected={i === activeIndex}
              className={cn(
                "cursor-pointer rounded-md px-2 py-1.5",
                i === activeIndex ? "bg-muted" : "hover:bg-muted",
              )}
              // mousedown keeps input focus so the list does not blur-close first
              onMouseDown={(e) => {
                e.preventDefault();
                selectValue(o.value);
              }}
              onMouseEnter={() => setActiveIndex(i)}
            >
              {o.label}
            </li>
          ))}
          {showCreate && (
            <li
              role="option"
              aria-selected={activeIndex === filtered.length}
              className={cn(
                "cursor-pointer rounded-md px-2 py-1.5",
                activeIndex === filtered.length ? "bg-muted" : "hover:bg-muted",
              )}
              onMouseDown={(e) => {
                e.preventDefault();
                selectValue(trimmed);
              }}
              onMouseEnter={() => setActiveIndex(filtered.length)}
            >
              {t("combobox.create", { value: trimmed })}
            </li>
          )}
        </ul>
      )}
    </div>
  );
}
