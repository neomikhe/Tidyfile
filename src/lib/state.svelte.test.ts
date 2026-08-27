import { beforeEach, describe, expect, test, vi } from "vitest";
import type { ActivityEntry, PlannedChange, Rule, Settings } from "./ipc";

const backend = {
  rules: [] as Rule[],
  settings: { folders: [], watched: [], onCollision: "suffix" } as Settings,
  preview: [] as PlannedChange[],
  history: [] as ActivityEntry[],
  unfinished: [] as PlannedChange[],
  saved: 0,
  simulated: 0,
  failNext: null as { code: string; message: string } | null,
};

function maybeFail(): void {
  if (backend.failNext !== null) {
    const problem = backend.failNext;
    backend.failNext = null;
    throw problem;
  }
}

vi.mock("./ipc", async (original) => {
  const real = await original<typeof import("./ipc")>();
  return {
    ...real,
    loadRules: vi.fn(async () => backend.rules),
    saveRules: vi.fn(async (rules: Rule[]) => {
      maybeFail();
      backend.saved += 1;
      backend.rules = rules;
    }),
    loadSettings: vi.fn(async () => backend.settings),
    saveSettings: vi.fn(async (settings: Settings) => {
      backend.settings = settings;
    }),
    simulate: vi.fn(async () => {
      maybeFail();
      backend.simulated += 1;
      return backend.preview;
    }),
    organize: vi.fn(async () => ({ batch: "b1", applied: 1, skipped: 0, failed: 0 })),
    undo: vi.fn(async () => ({ batch: "b1", applied: 1, skipped: 0, failed: 0 })),
    undoOperation: vi.fn(async () => ({ batch: "b1", applied: 1, skipped: 0, failed: 0 })),
    operations: vi.fn(async () => []),
    activity: vi.fn(async () => backend.history),
    interrupted: vi.fn(async () => backend.unfinished),
    settleInterrupted: vi.fn(async () => backend.unfinished.length),
    startWatching: vi.fn(async () => undefined),
    stopWatching: vi.fn(async () => undefined),
    watchedFolders: vi.fn(async () => backend.settings.watched),
    folderStatus: vi.fn(async (folders: string[]) =>
      folders.map((folder) => ({ folder, state: "ok" as const })),
    ),
  };
});

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => undefined),
}));

const { Workspace } = await import("./state.svelte");

function aRule(id: string, enabled: boolean): Rule {
  return {
    id,
    name: `rule ${id}`,
    enabled,
    combinator: "all",
    conditions: [{ type: "extension", anyOf: ["pdf"] }],
    actions: [{ type: "moveTo", folder: "/out" }],
  };
}

beforeEach(() => {
  backend.rules = [];
  backend.settings = { folders: [], watched: [], onCollision: "suffix" };
  backend.preview = [];
  backend.history = [];
  backend.unfinished = [];
  backend.saved = 0;
  backend.simulated = 0;
  backend.failNext = null;
});

describe("startup", () => {
  test("loads rules, settings and unfinished work", async () => {
    backend.rules = [aRule("a", true)];
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "skip" };
    backend.unfinished = [{ kind: "move", source: "/a", destination: "/b" }];
    const workspace = new Workspace();

    await workspace.initialise();

    expect(workspace.rules).toHaveLength(1);
    expect(workspace.folders).toEqual(["/watched"]);
    expect(workspace.onCollision).toBe("skip");
    expect(workspace.unfinished).toHaveLength(1);
    expect(workspace.status.kind).toBe("idle");
  });
});

describe("running", () => {
  test("cannot run without a folder", async () => {
    backend.rules = [aRule("a", true)];
    const workspace = new Workspace();
    await workspace.initialise();

    expect(workspace.canRun).toBe(false);
  });

  test("cannot run when every rule is disabled", async () => {
    backend.rules = [aRule("a", false)];
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix" };
    const workspace = new Workspace();
    await workspace.initialise();

    expect(workspace.canRun).toBe(false);
  });

  test("can run with a folder and an enabled rule", async () => {
    backend.rules = [aRule("a", true)];
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix" };
    const workspace = new Workspace();
    await workspace.initialise();

    expect(workspace.canRun).toBe(true);
  });

  test("only enabled rules are sent to the backend", async () => {
    backend.rules = [aRule("a", true), aRule("b", false)];
    const workspace = new Workspace();
    await workspace.initialise();

    expect(workspace.enabledRules).toHaveLength(1);
    expect(workspace.enabledRules[0]?.id).toBe("a");
  });
});

describe("editing", () => {
  test("editing does not save, so a keystroke costs no round trip", async () => {
    backend.rules = [aRule("a", true)];
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix" };
    const workspace = new Workspace();
    await workspace.initialise();
    const savesBefore = backend.saved;
    const simulationsBefore = backend.simulated;

    workspace.edit({ ...aRule("a", true), name: "renamed" });

    expect(backend.saved).toBe(savesBefore);
    expect(backend.simulated).toBe(simulationsBefore);
    expect(workspace.rules[0]?.name).toBe("renamed");
  });

  test("editing invalidates the preview so a stale one is never shown", async () => {
    backend.rules = [aRule("a", true)];
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix" };
    backend.preview = [{ kind: "move", source: "/a", destination: "/b" }];
    const workspace = new Workspace();
    await workspace.initialise();
    await workspace.refreshPreview();
    expect(workspace.preview).toHaveLength(1);

    workspace.edit({ ...aRule("a", true), name: "renamed" });

    expect(workspace.preview).toBeNull();
  });

  test("committing saves once and simulates once", async () => {
    backend.rules = [aRule("a", true)];
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix" };
    const workspace = new Workspace();
    await workspace.initialise();
    backend.saved = 0;
    backend.simulated = 0;

    await workspace.commit();

    expect(backend.saved).toBe(1);
    expect(backend.simulated).toBe(1);
  });

  test("toggling a rule persists immediately", async () => {
    backend.rules = [aRule("a", false)];
    const workspace = new Workspace();
    await workspace.initialise();
    backend.saved = 0;

    await workspace.toggle("a");

    expect(backend.saved).toBe(1);
    expect(workspace.rules[0]?.enabled).toBe(true);
  });

  test("removing a rule persists immediately", async () => {
    backend.rules = [aRule("a", true), aRule("b", true)];
    const workspace = new Workspace();
    await workspace.initialise();

    await workspace.remove("a");

    expect(workspace.rules.map((rule) => rule.id)).toEqual(["b"]);
  });
});

describe("folders", () => {
  test("adding the same folder twice keeps one entry", async () => {
    const workspace = new Workspace();
    await workspace.initialise();

    await workspace.addFolder("/watched");
    await workspace.addFolder("/watched");

    expect(workspace.folders).toEqual(["/watched"]);
  });

  test("removing a folder leaves the others", async () => {
    backend.settings = { folders: ["/one", "/two"], watched: [], onCollision: "suffix" };
    const workspace = new Workspace();
    await workspace.initialise();

    await workspace.removeFolder("/one");

    expect(workspace.folders).toEqual(["/two"]);
  });
});

describe("failures", () => {
  test("a backend error becomes a readable problem and clears the preview", async () => {
    backend.rules = [aRule("a", true)];
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix" };
    const workspace = new Workspace();
    await workspace.initialise();
    backend.failNext = { code: "forbiddenFolder", message: "this folder cannot be watched" };

    await workspace.refreshPreview();

    expect(workspace.status).toEqual({
      kind: "problem",
      message: "this folder cannot be watched",
    });
    expect(workspace.preview).toBeNull();
  });

  test("an unrecognised failure still yields a message rather than crashing", async () => {
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix" };
    const workspace = new Workspace();
    await workspace.initialise();
    backend.failNext = { code: 42, message: undefined } as never;

    await workspace.refreshPreview();

    expect(workspace.status.kind).toBe("problem");
  });

  test("a later success clears the problem", async () => {
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix" };
    const workspace = new Workspace();
    await workspace.initialise();
    backend.failNext = { code: "folderNotFound", message: "gone" };
    await workspace.refreshPreview();
    expect(workspace.status.kind).toBe("problem");

    await workspace.refreshPreview();

    expect(workspace.status.kind).toBe("idle");
  });
});

describe("watching", () => {
  test("a folder can be watched on its own, leaving the others alone", async () => {
    backend.settings = { folders: ["/one", "/two"], watched: [], onCollision: "suffix" };
    const workspace = new Workspace();
    await workspace.initialise();

    await workspace.setWatched("/one", true);

    expect(workspace.isWatched("/one")).toBe(true);
    expect(workspace.isWatched("/two")).toBe(false);
    expect(backend.settings.watched).toEqual(["/one"]);
  });

  test("unwatching the last folder stops watching altogether", async () => {
    backend.settings = { folders: ["/one"], watched: ["/one"], onCollision: "suffix" };
    const workspace = new Workspace();
    await workspace.initialise();

    await workspace.setWatched("/one", false);

    expect(workspace.watched).toEqual([]);
  });

  test("removing a folder also stops watching it", async () => {
    backend.settings = { folders: ["/one", "/two"], watched: ["/one"], onCollision: "suffix" };
    const workspace = new Workspace();
    await workspace.initialise();

    await workspace.removeFolder("/one");

    expect(workspace.folders).toEqual(["/two"]);
    expect(workspace.isWatched("/one")).toBe(false);
  });
});

describe("collision policy", () => {
  test("choosing a policy persists it", async () => {
    const workspace = new Workspace();
    await workspace.initialise();

    await workspace.setCollision("skip");

    expect(workspace.onCollision).toBe("skip");
    expect(backend.settings.onCollision).toBe("skip");
  });
});
