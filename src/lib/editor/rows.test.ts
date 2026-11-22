import { describe, expect, it } from "vitest";
import { addedAfter, hasProblem, keyed, replaced, valuesOf, without, type Keyed } from "./rows";

function keysOf<T>(rows: Keyed<T>[]): number[] {
  return rows.map((row) => row.key);
}

function rowWith<T>(rows: Keyed<T>[], value: T): Keyed<T> {
  const row = rows.find((candidate) => candidate.value === value);
  if (row === undefined) throw new Error(`no row holds ${String(value)}`);
  return row;
}

describe("editor rows", () => {
  it("never hands out the same key twice", () => {
    const keys = [...keysOf(keyed(["a", "b", "c"])), ...keysOf(keyed(["d", "e"]))];
    expect(new Set(keys).size).toBe(5);
  });

  it("removing a row keeps every other row with its own key", () => {
    const rows = keyed(["log", "pdf", "zip"]);
    const rest = without(rows, rowWith(rows, "log").key);
    expect(valuesOf(rest)).toEqual(["pdf", "zip"]);
    expect(keysOf(rest)).toEqual([rowWith(rows, "pdf").key, rowWith(rows, "zip").key]);
  });

  it("a new row lands right after the one it was added from, with a key of its own", () => {
    const rows = keyed(["log", "pdf"]);
    const next = addedAfter(rows, rowWith(rows, "log").key, "zip");
    expect(valuesOf(next)).toEqual(["log", "zip", "pdf"]);
    expect(new Set(keysOf(next)).size).toBe(3);
  });

  it("changing a row keeps its key and touches no other row", () => {
    const rows = keyed(["log", "pdf"]);
    const next = replaced(rows, rowWith(rows, "pdf").key, "zip");
    expect(valuesOf(next)).toEqual(["log", "zip"]);
    expect(keysOf(next)).toEqual(keysOf(rows));
  });

  it("a pattern problem follows its row when an earlier row is removed", () => {
    const rows = keyed(["ext", "(bad"]);
    const rest = without(rows, rowWith(rows, "ext").key);
    expect(hasProblem(rest, { [rowWith(rows, "(bad").key]: true })).toBe(true);
  });

  it("a removed row's pattern problem no longer blocks saving", () => {
    const rows = keyed(["(bad", "pdf"]);
    const rest = without(rows, rowWith(rows, "(bad").key);
    expect(hasProblem(rest, { [rowWith(rows, "(bad").key]: true })).toBe(false);
  });
});
