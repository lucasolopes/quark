import { useContext } from "react";
import { I18nContext, type I18nContextValue } from "./shared";

function useI18n(): I18nContextValue {
  const ctx = useContext(I18nContext);
  if (!ctx) throw new Error("useI18n/useT/useLocale must be used within an I18nProvider");
  return ctx;
}

/** Returns the translation function `t(key, params?)`, type-safe on `key`. */
export function useT() {
  return useI18n().t;
}

/** Returns the current locale and a setter that persists the choice to localStorage. */
export function useLocale() {
  const { locale, setLocale } = useI18n();
  return { locale, setLocale };
}
