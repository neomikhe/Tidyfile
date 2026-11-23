import { describe, expect, it } from "vitest";
import { latestOnly } from "./latest";

function gated() {
  const calls: string[] = [];
  let release: () => void = () => undefined;
  const work = async (input: string): Promise<string> => {
    calls.push(input);
    if (calls.length === 1) {
      await new Promise<void>((resolve) => {
        release = resolve;
      });
    }
    return `done ${input}`;
  };
  return { calls, work, release: () => release() };
}

describe("latestOnly", () => {
  it("runs one call at a time and keeps only the newest request that arrived meanwhile", async () => {
    const { calls, work, release } = gated();
    const run = latestOnly(work);

    const first = run("a");
    const second = run("b");
    const third = run("c");
    release();

    expect(await first).toBe("done a");
    expect(await second).toBeNull();
    expect(await third).toBe("done c");
    expect(calls).toEqual(["a", "c"]);
  });

  it("starts at once when nothing is running", async () => {
    const run = latestOnly(async (input: number) => input * 2);

    expect(await run(2)).toBe(4);
    expect(await run(3)).toBe(6);
  });
});
