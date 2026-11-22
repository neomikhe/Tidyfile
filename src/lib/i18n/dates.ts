import { i18n, t } from "./index.svelte";

const DAY_MS = 86_400_000;

export function formatTime(seconds: number): string {
  return new Intl.DateTimeFormat(i18n.locale, { timeStyle: "short" }).format(new Date(seconds * 1000));
}

function startOfDay(moment: Date): number {
  return new Date(moment.getFullYear(), moment.getMonth(), moment.getDate()).getTime();
}

export function dayLabel(seconds: number, now: Date): string {
  const day = startOfDay(new Date(seconds * 1000));
  const daysAgo = Math.round((startOfDay(now) - day) / DAY_MS);
  if (daysAgo === 0) {
    return t("activity.today");
  }
  if (daysAgo === 1) {
    return t("activity.yesterday");
  }
  return new Intl.DateTimeFormat(i18n.locale, { dateStyle: "long" }).format(day);
}
