import { beforeEach, expect, test } from "vitest";
import { i18n } from "./index.svelte";
import { describeError } from "./errors";
import { plain } from "./testing";

beforeEach(() => {
  i18n.locale = "es";
});

test("a backend error is told in the chosen language by its code", () => {
  expect(describeError({ code: "forbiddenFolder", message: "the folder cannot be used" })).toContain(
    "carpeta del sistema",
  );
  i18n.locale = "en";
  expect(describeError({ code: "planChanged", message: "x" })).toBe("The folder changed since the preview.");
});

test("a rule error names the rule", () => {
  expect(plain(describeError({ code: "invalidRule", message: "x", detail: "Facturas" }))).toContain("«Facturas»");
  expect(describeError({ code: "invalidRule", message: "x" })).toBe(
    "Una regla tiene un error y no se puede evaluar.",
  );
});

test("anything unknown gets a generic message", () => {
  expect(describeError({ code: "zzz", message: "internal detail" })).toBe("Algo ha fallado. Inténtalo de nuevo.");
  expect(describeError("texto")).toBe("Algo ha fallado. Inténtalo de nuevo.");
});
