import type { Action, ActivityEntry, Condition, Rule } from "./ipc";
import { dayLabel, i18n, t, tn, type MessageKey } from "./i18n/index.svelte";

export type SizeUnit = "KB" | "MB" | "GB";

const UNITS: SizeUnit[] = ["KB", "MB", "GB"];

const KNOWN_FOLDERS: Record<string, MessageKey> = {
  downloads: "folder.downloads",
  documents: "folder.documents",
  pictures: "folder.pictures",
  music: "folder.music",
  videos: "folder.videos",
  desktop: "folder.desktop",
};

function lastSegment(path: string): string {
  const parts = path.split(/[\\/]/).filter((part) => part.length > 0);
  return parts[parts.length - 1] ?? path;
}

export function knownFolderLabel(path: string): string {
  const name = lastSegment(path);
  const key = KNOWN_FOLDERS[name.toLowerCase()];
  return key ? t(key) : name;
}

export function watchingLabel(watched: string[]): string {
  const [only] = watched;
  if (only === undefined) {
    return t("watching.off");
  }
  return watched.length === 1 ? t("watching.one", { folder: knownFolderLabel(only) }) : tn("watching.many", watched.length);
}

export function toBytes(amount: number, unit: SizeUnit): number {
  return Math.round(amount * 1024 ** (UNITS.indexOf(unit) + 1));
}

export function fromBytes(bytes: number): { amount: number; unit: SizeUnit } {
  const index = UNITS.findLastIndex((_, at) => bytes >= 1024 ** (at + 1));
  const unitIndex = Math.max(index, 0);
  const amount = Math.round((bytes / 1024 ** (unitIndex + 1)) * 100) / 100;
  return { amount, unit: UNITS[unitIndex] ?? "KB" };
}

function formatSize(bytes: number): string {
  const { amount, unit } = fromBytes(bytes);
  return `${new Intl.NumberFormat(i18n.locale, { maximumFractionDigits: 2 }).format(amount)} ${unit}`;
}

function extensions(list: string[]): string {
  return list.map((item) => item.replace(/^\./, "").toUpperCase()).join(t("join.or"));
}

type PhraseFor = { [K in Condition["type"]]: (condition: Extract<Condition, { type: K }>) => string };

const CONDITION_PHRASES: PhraseFor = {
  extension: (condition) => t("phrase.extension", { list: extensions(condition.anyOf) }),
  nameContains: (condition) => t("phrase.nameContains", { text: condition.text }),
  nameMatchesGlob: (condition) => t("phrase.nameMatchesGlob", { pattern: condition.pattern }),
  nameMatchesRegex: (condition) => t("phrase.nameMatchesRegex", { pattern: condition.pattern }),
  largerThan: (condition) => t("phrase.largerThan", { size: formatSize(condition.bytes) }),
  smallerThan: (condition) => t("phrase.smallerThan", { size: formatSize(condition.bytes) }),
  olderThan: (condition) => tn("phrase.olderThan", condition.days),
  newerThan: (condition) => tn("phrase.newerThan", condition.days),
  inSubfolder: (condition) => t("phrase.inSubfolder", { name: condition.name }),
};

function conditionPhrase(condition: Condition): string {
  const phrase = CONDITION_PHRASES[condition.type] as (condition: Condition) => string;
  return phrase(condition);
}

function destination(folder: string, subfolder: string | undefined): string {
  if (folder.trim().length === 0) {
    return t("phrase.noFolder");
  }
  const below = (subfolder ?? "").split(/[\\/]/).filter((part) => part.length > 0);
  return [knownFolderLabel(folder), ...below].join(" › ");
}

function actionPhrase(action: Action): string {
  switch (action.type) {
    case "moveTo":
    case "copyTo": {
      const path = destination(action.folder, action.subfolder);
      const placed = action.rename ? t("phrase.renamedAs", { path, template: action.rename }) : path;
      return action.type === "copyTo" ? t("phrase.copyTo", { path: placed }) : placed;
    }
    case "renameTo":
      return t("phrase.renameTo", { template: action.template });
    case "trash":
      return t("phrase.trash");
  }
}

function capitalise(text: string): string {
  return text.charAt(0).toLocaleUpperCase(i18n.locale) + text.slice(1);
}

export function describeRule(rule: Rule): string {
  const joiner = rule.combinator === "any" ? t("join.or") : t("join.and");
  const conditions = capitalise(rule.conditions.map(conditionPhrase).join(joiner));
  const actions = rule.actions.map(actionPhrase).join(" · ");
  return `${conditions} → ${actions}`;
}

export function batchCounts(entry: ActivityEntry): string {
  const parts = [tn("count.tidied", entry.done + entry.undone)];
  if (entry.undone > 0) {
    parts.push(tn("count.undone", entry.undone));
  }
  if (entry.failed > 0) {
    parts.push(tn("count.failed", entry.failed));
  }
  if (entry.skipped > 0) {
    parts.push(tn("count.skipped", entry.skipped));
  }
  return parts.join(" · ");
}

export function groupByDay<T extends { recordedAt: number }>(
  entries: T[],
  now: Date,
): { label: string; entries: T[] }[] {
  const groups: { label: string; entries: T[] }[] = [];
  for (const entry of entries) {
    const label = dayLabel(entry.recordedAt, now);
    const last = groups[groups.length - 1];
    if (last?.label === label) {
      last.entries.push(entry);
    } else {
      groups.push({ label, entries: [entry] });
    }
  }
  return groups;
}
