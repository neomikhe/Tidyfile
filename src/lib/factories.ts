import type { Action, Combinator, Condition, Rule } from "./ipc";
import type { MessageKey } from "./i18n/index.svelte";

export type ConditionType = Condition["type"];
export type ActionType = Action["type"];

export const conditionTypes: ConditionType[] = [
  "extension",
  "nameContains",
  "nameMatchesGlob",
  "nameMatchesRegex",
  "largerThan",
  "smallerThan",
  "olderThan",
  "newerThan",
  "inSubfolder",
];

export const actionTypes: ActionType[] = ["moveTo", "copyTo", "renameTo", "trash"];

export const conditionLabels: Record<ConditionType, MessageKey> = {
  extension: "condition.extension",
  nameContains: "condition.nameContains",
  nameMatchesGlob: "condition.nameMatchesGlob",
  nameMatchesRegex: "condition.nameMatchesRegex",
  largerThan: "condition.largerThan",
  smallerThan: "condition.smallerThan",
  olderThan: "condition.olderThan",
  newerThan: "condition.newerThan",
  inSubfolder: "condition.inSubfolder",
};

export const actionLabels: Record<ActionType, MessageKey> = {
  moveTo: "action.moveTo",
  copyTo: "action.copyTo",
  renameTo: "action.renameTo",
  trash: "action.trash",
};

export const combinators: Combinator[] = ["all", "any"];

export const PLACEHOLDERS: { token: string; label: MessageKey }[] = [
  { token: "{name}", label: "chip.name" },
  { token: "{ext}", label: "chip.ext" },
  { token: "{date}", label: "chip.date" },
  { token: "{year}", label: "chip.year" },
  { token: "{month}", label: "chip.month" },
  { token: "{day}", label: "chip.day" },
  { token: "{counter}", label: "chip.counter" },
];

export function blankCondition(type: ConditionType): Condition {
  switch (type) {
    case "extension":
      return { type, anyOf: [] };
    case "nameContains":
      return { type, text: "" };
    case "nameMatchesGlob":
    case "nameMatchesRegex":
      return { type, pattern: "" };
    case "largerThan":
    case "smallerThan":
      return { type, bytes: 0 };
    case "olderThan":
    case "newerThan":
      return { type, days: 0 };
    case "inSubfolder":
      return { type, name: "" };
  }
}

export function blankAction(type: ActionType): Action {
  switch (type) {
    case "moveTo":
    case "copyTo":
      return { type, folder: "" };
    case "renameTo":
      return { type, template: "{name}.{ext}" };
    case "trash":
      return { type };
  }
}

export function blankRule(): Rule {
  return {
    id: crypto.randomUUID(),
    name: "New rule",
    enabled: false,
    combinator: "all",
    conditions: [blankCondition("extension")],
    actions: [blankAction("moveTo")],
  };
}

export function extensionsToText(values: string[]): string {
  return values.join(", ");
}

export function textToExtensions(text: string): string[] {
  return text
    .split(",")
    .map((piece) => piece.trim().replace(/^\./, ""))
    .filter((piece) => piece.length > 0);
}

export function logicChanged(before: Rule, after: Rule): boolean {
  const logic = (rule: Rule): string =>
    JSON.stringify([rule.combinator, rule.conditions, rule.actions]);
  return logic(before) !== logic(after);
}

export function isIncomplete(rule: Rule): boolean {
  if (rule.conditions.length === 0 || rule.actions.length === 0) {
    return true;
  }
  return rule.conditions.some(conditionIsBlank) || rule.actions.some(actionIsBlank);
}

function conditionIsBlank(condition: Condition): boolean {
  switch (condition.type) {
    case "extension":
      return condition.anyOf.length === 0;
    case "nameContains":
      return condition.text.trim().length === 0;
    case "nameMatchesGlob":
    case "nameMatchesRegex":
      return condition.pattern.trim().length === 0;
    case "inSubfolder":
      return condition.name.trim().length === 0;
    default:
      return false;
  }
}

function actionIsBlank(action: Action): boolean {
  switch (action.type) {
    case "moveTo":
    case "copyTo":
      return action.folder.trim().length === 0;
    case "renameTo":
      return action.template.trim().length === 0;
    case "trash":
      return false;
  }
}
