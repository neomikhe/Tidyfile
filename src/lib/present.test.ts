import { beforeEach, describe, expect, test } from "vitest";
import type { ActivityEntry, Rule } from "./ipc";
import { i18n } from "./i18n/index.svelte";
import {
  batchCounts,
  describeRule,
  fileName,
  fromBytes,
  groupByDay,
  knownFolderLabel,
  showable,
  toBytes,
  watchingLabel,
} from "./present";
import { detectPlatform } from "./platform";
import { plain } from "./i18n/testing";

function rule(changes: Partial<Rule>): Rule {
  return {
    id: "r",
    name: "Facturas",
    enabled: true,
    combinator: "all",
    conditions: [],
    actions: [],
    ...changes,
  };
}

const invoices = rule({
  conditions: [
    { type: "nameContains", text: "factura" },
    { type: "extension", anyOf: ["pdf"] },
  ],
  actions: [{ type: "moveTo", folder: "D:\\Data\\Documents", subfolder: "Facturas/{year}" }],
});

function entry(changes: Partial<ActivityEntry>): ActivityEntry {
  return { batch: "b", done: 0, undone: 0, failed: 0, skipped: 0, recordedAt: 0, origin: null, folder: null, ...changes };
}

beforeEach(() => {
  i18n.locale = "es";
});

describe("describeRule", () => {
  test("reads a rule as one sentence", () => {
    expect(plain(describeRule(invoices))).toBe("Nombre contiene «factura» y es PDF → Documentos › Facturas › {year}");
  });

  test("any joins the conditions with or", () => {
    const loose = rule({
      combinator: "any",
      conditions: [
        { type: "extension", anyOf: ["msi", "exe"] },
        { type: "olderThan", days: 30 },
      ],
      actions: [{ type: "trash" }],
    });
    expect(plain(describeRule(loose))).toBe("Es MSI o EXE o tiene más de 30 días → Papelera");
  });

  test("several actions are separated by a dot", () => {
    const both = rule({
      conditions: [{ type: "largerThan", bytes: 10 * 1024 * 1024 }],
      actions: [{ type: "copyTo", folder: "D:\\Backup" }, { type: "trash" }],
    });
    expect(plain(describeRule(both))).toBe("Pesa más de 10 MB → copia en Backup · Papelera");
  });

  test("the same rule reads in English", () => {
    i18n.locale = "en";
    expect(plain(describeRule(invoices))).toBe("Name contains “factura” and is PDF → Documents › Facturas › {year}");
  });
});

describe("folder names", () => {
  test("well-known folders show their name in the chosen language", () => {
    expect(knownFolderLabel("D:\\Data\\Downloads")).toBe("Descargas");
    expect(knownFolderLabel("\\\\?\\D:\\Data\\downloads\\")).toBe("Descargas");
    expect(knownFolderLabel("D:\\Escáner")).toBe("Escáner");
    i18n.locale = "en";
    expect(knownFolderLabel("D:\\Data\\Downloads")).toBe("Downloads");
  });

  test("the watching label counts the folders", () => {
    expect(watchingLabel([])).toBe("Vigilancia apagada");
    expect(plain(watchingLabel(["D:\\Data\\Downloads"]))).toBe("Vigilando Descargas");
    expect(watchingLabel(["C:\\a", "D:\\b"])).toBe("Vigilando 2 carpetas");
  });
});

describe("names on screen", () => {
  test("a direction control inside a file name is shown instead of obeyed", () => {
    expect(showable("factura\u202Efdp.exe")).toBe("factura[U+202E]fdp.exe");
    expect(showable("informe año.pdf")).toBe("informe año.pdf");
  });

  test("the file name is the last part of the path, made showable", () => {
    expect(fileName("D:\\Data\\Downloads\\a\u2066b.pdf")).toBe("a[U+2066]b.pdf");
    expect(fileName("/home/u/notas")).toBe("notas");
  });
});

describe("sizes", () => {
  test("units convert to bytes and back", () => {
    expect(toBytes(10, "KB")).toBe(10240);
    expect(fromBytes(10485760)).toEqual({ amount: 10, unit: "MB" });
    expect(fromBytes(1536)).toEqual({ amount: 1.5, unit: "KB" });
    expect(toBytes(fromBytes(3 * 1024 ** 3).amount, fromBytes(3 * 1024 ** 3).unit)).toBe(3 * 1024 ** 3);
  });
});

describe("activity", () => {
  test("a batch summary counts what was tidied, undone and skipped", () => {
    expect(batchCounts(entry({ done: 13 }))).toBe("13 archivos ordenados");
    expect(batchCounts(entry({ done: 13, undone: 5 }))).toBe("18 archivos ordenados · 5 deshechos");
    expect(batchCounts(entry({ done: 2, skipped: 1 }))).toBe("2 archivos ordenados · 1 omitido");
  });

  test("batches are grouped by day in order", () => {
    const now = new Date(2026, 10, 19, 12, 0);
    const at = (day: number, hour: number): number => Math.floor(new Date(2026, 10, day, hour).getTime() / 1000);
    const groups = groupByDay([entry({ batch: "a", recordedAt: at(19, 11) }), entry({ batch: "b", recordedAt: at(19, 9) }), entry({ batch: "c", recordedAt: at(18, 20) })], now);

    expect(groups.map((group) => [group.label, group.entries.map((item) => item.batch)])).toEqual([
      ["Hoy", ["a", "b"]],
      ["Ayer", ["c"]],
    ]);
  });
});

describe("platform", () => {
  test("the web view tells which system it runs on", () => {
    expect(detectPlatform("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Edg/131.0")).toBe("windows");
    expect(detectPlatform("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15")).toBe("macos");
    expect(detectPlatform("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15")).toBe("linux");
  });
});
