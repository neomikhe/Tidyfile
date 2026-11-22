import { isIpcError, type ErrorCode } from "../ipc";
import { t, type MessageKey } from "./index.svelte";

const MESSAGES: Record<Exclude<ErrorCode, "invalidRule">, MessageKey> = {
  forbiddenFolder: "error.forbiddenFolder",
  notAFolder: "error.notAFolder",
  folderNotFound: "error.folderNotFound",
  historyUnavailable: "error.historyUnavailable",
  executionFailed: "error.executionFailed",
  rulesUnreachable: "error.rulesUnreachable",
  rulesMalformed: "error.rulesMalformed",
  watchFailed: "error.watchFailed",
  planChanged: "error.planChanged",
  invalidPattern: "error.invalidPattern",
  unavailable: "error.unavailable",
};

function isKnown(code: string): code is keyof typeof MESSAGES {
  return Object.hasOwn(MESSAGES, code);
}

export function describeError(error: unknown): string {
  if (!isIpcError(error)) {
    return t("error.generic");
  }
  if (error.code === "invalidRule") {
    return error.detail ? t("error.invalidRule", { rule: error.detail }) : t("error.invalidRuleUnnamed");
  }
  return isKnown(error.code) ? t(MESSAGES[error.code]) : t("error.generic");
}
