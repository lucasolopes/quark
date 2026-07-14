import { useCallback, useMemo, useState, type ReactNode } from "react";
import { I18nContext, MESSAGES, STORAGE_KEY, getMessage, interpolate, resolveDefaultLocale, type I18nContextValue, type Locale, type MessageKey, type Params } from "./shared";

interface I18nProviderProps {
  children: ReactNode;
  /** Forces the initial locale, bypassing localStorage/navigator detection — used in tests for determinism. */
  locale?: Locale;
}

export function I18nProvider({ children, locale: forcedLocale }: I18nProviderProps) {
  const [locale, setLocaleState] = useState<Locale>(() => forcedLocale ?? resolveDefaultLocale());

  const setLocale = useCallback((next: Locale) => {
    setLocaleState(next);
    if (typeof localStorage !== "undefined") localStorage.setItem(STORAGE_KEY, next);
  }, []);

  const t = useCallback(
    (key: MessageKey, params?: Params) => interpolate(getMessage(MESSAGES[locale], key), params),
    [locale],
  );

  const value = useMemo<I18nContextValue>(() => ({ locale, setLocale, t }), [locale, setLocale, t]);

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}
