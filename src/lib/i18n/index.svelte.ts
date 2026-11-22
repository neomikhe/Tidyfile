import type { Language } from "../ipc";
import { es, type Messages } from "./es";
import { en } from "./en";

export type Locale = "es" | "en";

type Plural = { one: string; other: string };
type Params = Record<string, string | number>;

export type MessageKey = {
  [K in keyof Messages]: Messages[K] extends string ? K : never;
}[keyof Messages];

export type PluralKey = {
  [K in keyof Messages]: Messages[K] extends Plural ? K : never;
}[keyof Messages];

const catalogs: Record<Locale, Messages> = { es, en };

export const i18n = $state({ locale: "en" as Locale });

export function resolveLocale(language: Language, systemLanguage: string): Locale {
  if (language !== "auto") {
    return language;
  }
  return systemLanguage.toLowerCase().startsWith("es") ? "es" : "en";
}

export function applyLanguage(language: Language): void {
  i18n.locale = resolveLocale(language, globalThis.navigator?.language ?? "en");
  if (typeof document !== "undefined") {
    document.documentElement.lang = i18n.locale;
  }
}

function fill(text: string, params: Params = {}): string {
  return text.replace(/\{(\w+)\}/g, (whole: string, name: string) =>
    name in params ? String(params[name]) : whole,
  );
}

export function t(key: MessageKey, params?: Params): string {
  return fill(catalogs[i18n.locale][key] as string, params);
}

export function tn(key: PluralKey, count: number, params: Params = {}): string {
  const forms = catalogs[i18n.locale][key] as Plural;
  const form = new Intl.PluralRules(i18n.locale).select(count) === "one" ? forms.one : forms.other;
  return fill(form, { ...params, count: new Intl.NumberFormat(i18n.locale).format(count) });
}

export { dayLabel, formatTime } from "./dates";
