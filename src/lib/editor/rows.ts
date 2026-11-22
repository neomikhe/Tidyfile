export interface Keyed<T> {
  key: number;
  value: T;
}

let lastKey = 0;

export function keyed<T>(values: T[]): Keyed<T>[] {
  return values.map((value) => ({ key: ++lastKey, value }));
}

export function valuesOf<T>(rows: Keyed<T>[]): T[] {
  return rows.map((row) => row.value);
}

export function replaced<T>(rows: Keyed<T>[], key: number, value: T): Keyed<T>[] {
  return rows.map((row) => (row.key === key ? { key, value } : row));
}

export function without<T>(rows: Keyed<T>[], key: number): Keyed<T>[] {
  return rows.filter((row) => row.key !== key);
}

export function addedAfter<T>(rows: Keyed<T>[], key: number, value: T): Keyed<T>[] {
  const at = rows.findIndex((row) => row.key === key);
  return [...rows.slice(0, at + 1), ...keyed([value]), ...rows.slice(at + 1)];
}

export function hasProblem<T>(rows: Keyed<T>[], problems: Record<number, boolean>): boolean {
  return rows.some((row) => problems[row.key] === true);
}
