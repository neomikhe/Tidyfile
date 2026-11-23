import { beforeEach, describe, expect, test } from "vitest";
import { es } from "./es";
import { en } from "./en";
import { dayLabel, i18n, resolveLocale, t, tn } from "./index.svelte";
import { plain } from "./testing";

function placeholders(value: unknown): string[] {
  const texts = typeof value === "string" ? [value] : Object.values(value as Record<string, string>);
  return [...new Set(texts.flatMap((text) => [...text.matchAll(/\{(\w+)\}/g)].map((match) => match[1] ?? "")))].sort();
}

beforeEach(() => {
  i18n.locale = "es";
});

describe("dictionaries", () => {
  test("both languages have exactly the same keys", () => {
    expect(Object.keys(en).sort()).toEqual(Object.keys(es).sort());
  });

  test("every message uses the same placeholders in both languages", () => {
    for (const key of Object.keys(es) as (keyof typeof es)[]) {
      expect(placeholders(en[key]), key).toEqual(placeholders(es[key]));
    }
  });
});

describe("choosing the language", () => {
  test("auto follows the system language", () => {
    expect(resolveLocale("auto", "es-MX")).toBe("es");
    expect(resolveLocale("auto", "en-GB")).toBe("en");
    expect(resolveLocale("auto", "fr-FR")).toBe("en");
    expect(resolveLocale("es", "en-US")).toBe("es");
    expect(resolveLocale("en", "es-ES")).toBe("en");
  });
});

describe("texts", () => {
  test("t interpolates and tn picks the plural form", () => {
    expect(plain(t("watching.one", { folder: "Descargas" }))).toBe("Vigilando Descargas");
    expect(tn("count.tidied", 1)).toBe("1 archivo ordenado");
    expect(tn("count.tidied", 13)).toBe("13 archivos ordenados");
    i18n.locale = "en";
    expect(tn("count.tidied", 13)).toBe("13 files tidied");
  });

  test("a value put into a sentence is isolated, so its direction cannot reorder the sentence", () => {
    expect(t("watching.one", { folder: "אב" })).toBe("Vigilando ⁨אב⁩");
    expect(tn("count.tidied", 13)).toBe("13 archivos ordenados");
  });

  test("a placeholder without a value is left as it is", () => {
    expect(t("editor.subfolderPlaceholder")).toContain("{year}");
  });

  test("dayLabel says today and yesterday", () => {
    const now = new Date(2026, 10, 19, 10, 30);
    const seconds = (date: Date): number => Math.floor(date.getTime() / 1000);

    expect(dayLabel(seconds(new Date(2026, 10, 19, 0, 5)), now)).toBe("Hoy");
    expect(dayLabel(seconds(new Date(2026, 10, 18, 23, 59)), now)).toBe("Ayer");
    i18n.locale = "en";
    expect(dayLabel(seconds(new Date(2026, 10, 18, 8, 0)), now)).toBe("Yesterday");
    expect(dayLabel(seconds(new Date(2026, 10, 2, 8, 0)), now)).toContain("2026");
  });
});
