import { invoke } from "@tauri-apps/api/core";

export type Combinator = "all" | "any";

export type Condition =
  | { type: "extension"; anyOf: string[] }
  | { type: "nameContains"; text: string }
  | { type: "nameMatchesGlob"; pattern: string }
  | { type: "nameMatchesRegex"; pattern: string }
  | { type: "largerThan"; bytes: number }
  | { type: "smallerThan"; bytes: number }
  | { type: "olderThan"; days: number }
  | { type: "newerThan"; days: number }
  | { type: "inSubfolder"; name: string };

export type Action =
  | { type: "moveTo"; folder: string; subfolder?: string; rename?: string }
  | { type: "copyTo"; folder: string; subfolder?: string; rename?: string }
  | { type: "renameTo"; template: string }
  | { type: "trash" };

export interface Rule {
  id: string;
  name: string;
  enabled: boolean;
  combinator: Combinator;
  conditions: Condition[];
  actions: Action[];
}

export type ChangeKind = "move" | "copy" | "trash";

export interface PlannedChange {
  kind: ChangeKind;
  source: string;
  destination: string | null;
}

export interface BatchReport {
  batch: string;
  applied: number;
  skipped: number;
  failed: number;
}

export type ErrorCode =
  | "forbiddenFolder"
  | "notAFolder"
  | "folderNotFound"
  | "invalidRule"
  | "historyUnavailable"
  | "executionFailed"
  | "unavailable";

export interface IpcError {
  code: ErrorCode;
  message: string;
}

export function isIpcError(value: unknown): value is IpcError {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const candidate = value as Record<string, unknown>;
  return typeof candidate.code === "string" && typeof candidate.message === "string";
}

export function simulate(rules: Rule[], folder: string): Promise<PlannedChange[]> {
  return invoke<PlannedChange[]>("simulate", { rules, folder });
}

export function organize(rules: Rule[], folder: string): Promise<BatchReport> {
  return invoke<BatchReport>("organize", { rules, folder });
}

export function undo(batch: string): Promise<BatchReport> {
  return invoke<BatchReport>("undo", { batch });
}

export function interrupted(): Promise<PlannedChange[]> {
  return invoke<PlannedChange[]>("interrupted");
}
