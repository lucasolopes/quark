import { useLocale, useT } from "@/i18n";
import { cn } from "@/lib/utils";

interface LanguageSwitcherProps {
  className?: string;
}

/** EN / PT-BR toggle, persisted via `useLocale` (localStorage key `quark.lang`). */
export function LanguageSwitcher({ className }: LanguageSwitcherProps) {
  const { locale, setLocale } = useLocale();
  const t = useT();

  return (
    <div role="group" aria-label={t("common.languageSwitcherLabel")} className={cn("flex items-center gap-0.5", className)}>
      <button
        type="button"
        onClick={() => setLocale("en")}
        aria-pressed={locale === "en"}
        className={cn(
          "rounded-md px-2 py-1 text-xs font-semibold transition-colors",
          locale === "en" ? "bg-accent text-accent-foreground" : "text-muted-foreground hover:text-foreground",
        )}
      >
        EN
      </button>
      <button
        type="button"
        onClick={() => setLocale("pt-BR")}
        aria-pressed={locale === "pt-BR"}
        className={cn(
          "rounded-md px-2 py-1 text-xs font-semibold transition-colors",
          locale === "pt-BR" ? "bg-accent text-accent-foreground" : "text-muted-foreground hover:text-foreground",
        )}
      >
        PT
      </button>
    </div>
  );
}
