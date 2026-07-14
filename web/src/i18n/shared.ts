import { createContext } from "react";
import { en, type Messages } from "./en";
import { ptBR } from "./pt-BR";

export type Locale = "en" | "pt-BR";

export const STORAGE_KEY = "quark.lang";

export const MESSAGES: Record<Locale, Messages> = { en, "pt-BR": ptBR };

/** Union of every dotted path that leads to a string leaf in `Messages` (e.g. "links.heading"). */
type Paths<T> = T extends string
  ? never
  : {
      [K in keyof T & string]: T[K] extends string ? K : `${K}.${Paths<T[K]>}`;
    }[keyof T & string];

export type MessageKey = Paths<Messages>;

export type Params = Record<string, string | number>;

export function getMessage(messages: Messages, key: string): string {
  const value: unknown = key.split(".").reduce<unknown>((obj, part) => {
    if (obj && typeof obj === "object" && part in obj) return (obj as Record<string, unknown>)[part];
    return undefined;
  }, messages);
  return typeof value === "string" ? value : key;
}

export function interpolate(template: string, params?: Params): string {
  if (!params) return template;
  return template.replace(/\{(\w+)\}/g, (match, name: string) =>
    name in params ? String(params[name]) : match,
  );
}

export function resolveDefaultLocale(): Locale {
  if (typeof localStorage !== "undefined") {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored === "en" || stored === "pt-BR") return stored;
  }
  if (typeof navigator !== "undefined" && navigator.language.startsWith("pt")) return "pt-BR";
  return "en";
}

export interface I18nContextValue {
  locale: Locale;
  setLocale: (locale: Locale) => void;
  t: (key: MessageKey, params?: Params) => string;
}

export const I18nContext = createContext<I18nContextValue | null>(null);
