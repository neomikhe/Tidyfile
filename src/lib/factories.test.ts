import { describe, expect, test } from "vitest";
import {
  actionTypes,
  blankAction,
  blankCondition,
  blankRule,
  conditionTypes,
  extensionsToText,
  isIncomplete,
  textToExtensions,
} from "./factories";
import type { Rule } from "./ipc";

describe("extensions typed as free text", () => {
  test("a comma separated list becomes a list", () => {
    expect(textToExtensions("pdf, jpg")).toEqual(["pdf", "jpg"]);
  });

  test("a leading dot is stripped, since both forms are natural to type", () => {
    expect(textToExtensions(".pdf, .jpg")).toEqual(["pdf", "jpg"]);
  });

  test("stray spaces and empty entries are dropped", () => {
    expect(textToExtensions("  pdf ,, jpg ,")).toEqual(["pdf", "jpg"]);
  });

  test("an empty field means no extensions rather than one empty one", () => {
    expect(textToExtensions("")).toEqual([]);
    expect(textToExtensions("   ")).toEqual([]);
  });

  test("only one leading dot is stripped, so a doubled dot survives", () => {
    expect(textToExtensions("..pdf")).toEqual([".pdf"]);
  });

  test("the round trip through the text field is stable", () => {
    const typed = "pdf, jpg";
    expect(extensionsToText(textToExtensions(typed))).toBe(typed);
  });
});

describe("blank conditions", () => {
  test("every condition type produces its own shape", () => {
    for (const type of conditionTypes) {
      const made = blankCondition(type);
      expect(made.type).toBe(type);
    }
  });

  test("a blank extension condition holds an empty list, not an empty string", () => {
    const made = blankCondition("extension");
    expect(made).toEqual({ type: "extension", anyOf: [] });
  });

  test("numeric conditions start at zero rather than undefined", () => {
    expect(blankCondition("largerThan")).toEqual({ type: "largerThan", bytes: 0 });
    expect(blankCondition("olderThan")).toEqual({ type: "olderThan", days: 0 });
  });
});

describe("blank actions", () => {
  test("every action type produces its own shape", () => {
    for (const type of actionTypes) {
      expect(blankAction(type).type).toBe(type);
    }
  });

  test("a blank move has no folder yet", () => {
    expect(blankAction("moveTo")).toEqual({ type: "moveTo", folder: "" });
  });

  test("a blank rename starts from a template that already works", () => {
    expect(blankAction("renameTo")).toEqual({ type: "renameTo", template: "{name}.{ext}" });
  });

  test("trash carries nothing to fill in", () => {
    expect(blankAction("trash")).toEqual({ type: "trash" });
  });
});

describe("a brand new rule", () => {
  test("starts disabled, so it cannot act before it is finished", () => {
    expect(blankRule().enabled).toBe(false);
  });

  test("starts incomplete, so the checkbox stays locked", () => {
    expect(isIncomplete(blankRule())).toBe(true);
  });

  test("gets its own identifier", () => {
    expect(blankRule().id).not.toBe(blankRule().id);
  });
});

function ruleWith(overrides: Partial<Rule>): Rule {
  return {
    id: "r1",
    name: "a rule",
    enabled: false,
    combinator: "all",
    conditions: [{ type: "extension", anyOf: ["pdf"] }],
    actions: [{ type: "moveTo", folder: "/out" }],
    ...overrides,
  };
}

describe("when a rule counts as incomplete", () => {
  test("a filled rule is complete", () => {
    expect(isIncomplete(ruleWith({}))).toBe(false);
  });

  test("no conditions means incomplete, matching the engine refusing to match everything", () => {
    expect(isIncomplete(ruleWith({ conditions: [] }))).toBe(true);
  });

  test("no actions means incomplete", () => {
    expect(isIncomplete(ruleWith({ actions: [] }))).toBe(true);
  });

  test("an extension condition with no extensions is incomplete", () => {
    expect(isIncomplete(ruleWith({ conditions: [{ type: "extension", anyOf: [] }] }))).toBe(true);
  });

  test("text that is only whitespace does not count as filled in", () => {
    expect(isIncomplete(ruleWith({ conditions: [{ type: "nameContains", text: "   " }] }))).toBe(
      true,
    );
    expect(
      isIncomplete(ruleWith({ conditions: [{ type: "nameMatchesGlob", pattern: " " }] })),
    ).toBe(true);
  });

  test("a move without a folder is incomplete", () => {
    expect(isIncomplete(ruleWith({ actions: [{ type: "moveTo", folder: "" }] }))).toBe(true);
  });

  test("a trash action needs nothing, so it never blocks a rule", () => {
    expect(isIncomplete(ruleWith({ actions: [{ type: "trash" }] }))).toBe(false);
  });

  test("a numeric condition left at zero is a real choice, not an omission", () => {
    expect(isIncomplete(ruleWith({ conditions: [{ type: "largerThan", bytes: 0 }] }))).toBe(false);
  });
});
