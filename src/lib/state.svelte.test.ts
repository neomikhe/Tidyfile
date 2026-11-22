import { beforeEach, describe, expect, test, vi } from "vitest";
import type { ActivityEntry, PlannedChange, Rule, Settings } from "./ipc";

const backend = {
  rules: [] as Rule[],
  settings: { folders: [], watched: [], onCollision: "suffix", language: "auto" } as Settings,
  preview: [] as PlannedChange[],
  history: [] as ActivityEntry[],
  unfinished: [] as PlannedChange[],
  saved: 0,
  simulated: 0,
  organized: 0,
  settingsSaved: 0,
  settingsUnreachable: false,
  failNext: null as { code: string; message: string } | null,
  manualRestore: 0,
  notInTrash: 0,
  undoFailed: 0,
  alreadyGone: 0,
  unreachable: [] as string[],
  forbidden: [] as string[],
  statusFails: false,
  exampleCalls: [] as { rule: Rule; earlier: Rule[]; folders: string[] }[],
  report: { batch: "b1", applied: 1, skipped: 0, failed: 0, needsManualRestore: 0, notInTrash: 0, alreadyGone: 0 },
  gate: null as null | (() => void),
};

function waitAtGate(): Promise<void> {
  return new Promise((resolve) => {
    if (backend.gate === null) {
      resolve();
      return;
    }
    const previous = backend.gate;
    backend.gate = () => {
      previous();
      resolve();
    };
  });
}

function maybeFail(): void {
  if (backend.failNext !== null) {
    const problem = backend.failNext;
    backend.failNext = null;
    throw problem;
  }
}

function fakeStateOf(folder: string): "ok" | "unavailable" | "forbidden" {
  if (backend.forbidden.includes(folder)) {
    return "forbidden";
  }
  return backend.unreachable.includes(folder) ? "unavailable" : "ok";
}

vi.mock("./ipc", async (original) => {
  const real = await original<typeof import("./ipc")>();
  return {
    ...real,
    loadRules: vi.fn(async () => {
      maybeFail();
      return backend.rules;
    }),
    saveRules: vi.fn(async (rules: Rule[]) => {
      maybeFail();
      backend.saved += 1;
      backend.rules = rules;
    }),
    loadSettings: vi.fn(async () => {
      if (backend.settingsUnreachable) {
        throw { code: "rulesUnreachable", message: "the rule file could not be read or written" };
      }
      return backend.settings;
    }),
    saveSettings: vi.fn(async (settings: Settings) => {
      backend.settingsSaved += 1;
      backend.settings = settings;
    }),
    simulate: vi.fn(async () => {
      await waitAtGate();
      maybeFail();
      backend.simulated += 1;
      return backend.preview;
    }),
    organize: vi.fn(async (_rules: Rule[], _folders: string[], expected: PlannedChange[]) => {
      if (JSON.stringify(expected) !== JSON.stringify(backend.preview)) {
        throw { code: "planChanged", message: "the folder changed since the preview" };
      }
      backend.organized += 1;
      return backend.report;
    }),
    undo: vi.fn(async () => {
      maybeFail();
      return {
      batch: "b1",
      applied: 1,
      skipped: 0,
      failed: backend.undoFailed,
      needsManualRestore: backend.manualRestore,
      notInTrash: backend.notInTrash,
      alreadyGone: backend.alreadyGone,
      };
    }),
    undoOperation: vi.fn(async () => ({
      batch: "b1",
      applied: 0,
      skipped: backend.notInTrash,
      failed: backend.undoFailed,
      needsManualRestore: 0,
      notInTrash: backend.notInTrash,
      alreadyGone: backend.alreadyGone,
    })),
    operations: vi.fn(async () => []),
    activity: vi.fn(async () => backend.history),
    interrupted: vi.fn(async () => backend.unfinished),
    settleInterrupted: vi.fn(async () => backend.unfinished.length),
    startWatching: vi.fn(async () => undefined),
    stopWatching: vi.fn(async () => undefined),
    watchedFolders: vi.fn(async () => backend.settings.watched),
    exampleForRule: vi.fn(async (rule: Rule, earlier: Rule[], folders: string[]) => {
      backend.exampleCalls.push({ rule, earlier, folders });
      return { change: null, takenBy: null };
    }),
    folderStatus: vi.fn(async (folders: string[]) => {
      if (backend.statusFails) {
        throw { code: "unavailable", message: "the folder could not be checked" };
      }
      return folders.map((folder) => ({ folder, state: fakeStateOf(folder) }));
    }),
  };
});

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => undefined),
}));

vi.stubGlobal("navigator", { language: "en-US" });

const { Workspace } = await import("./state.svelte");
const { i18n } = await import("./i18n/index.svelte");
const { en } = await import("./i18n/en");

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
  i18n.locale = "en";
  vi.stubGlobal("navigator", { language: "en-US" });
  backend.rules = [];
  backend.settings = { folders: [], watched: [], onCollision: "suffix", language: "auto" };
  backend.preview = [];
  backend.history = [];
  backend.unfinished = [];
  backend.saved = 0;
  backend.simulated = 0;
  backend.organized = 0;
  backend.settingsSaved = 0;
  backend.settingsUnreachable = false;
  backend.failNext = null;
  backend.manualRestore = 0;
  backend.notInTrash = 0;
  backend.undoFailed = 0;
  backend.alreadyGone = 0;
  backend.unreachable = [];
  backend.forbidden = [];
  backend.statusFails = false;
  backend.exampleCalls = [];
  backend.report = { batch: "b1", applied: 1, skipped: 0, failed: 0, needsManualRestore: 0, notInTrash: 0, alreadyGone: 0 };
  backend.gate = null;
});

describe("startup", () => {
  test("loads rules, settings and unfinished work", async () => {
    backend.rules = [aRule("a", true)];
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "skip", language: "auto" };
    backend.unfinished = [{ kind: "move", source: "/a", destination: "/b" }];
    const workspace = new Workspace();

    await workspace.initialise();

    expect(workspace.rules).toHaveLength(1);
    expect(workspace.folders).toEqual(["/watched"]);
    expect(workspace.onCollision).toBe("skip");
    expect(workspace.unfinished).toHaveLength(1);
    expect(workspace.status.kind).toBe("idle");
  });

  test("an unreadable rule file still lets the folders and history load", async () => {
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "ask", language: "auto" };
    backend.history = [{ batch: "b1", done: 1, undone: 0, failed: 0, skipped: 0, recordedAt: 1, origin: null, folder: null }];
    backend.failNext = { code: "rulesMalformed", message: "the rule file is not valid Tidyfile JSON" };
    const workspace = new Workspace();

    await workspace.initialise();

    expect(workspace.folders).toEqual(["/watched"]);
    expect(workspace.onCollision).toBe("ask");
    expect(workspace.history).toHaveLength(1);
    expect(workspace.loadProblem).toBe(en["error.rulesMalformed"]);
  });

  test("a rule file that could not be read is never overwritten, and the warning stays", async () => {
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix", language: "auto" };
    backend.failNext = { code: "rulesUnreachable", message: "the rule file could not be read or written" };
    const workspace = new Workspace();
    await workspace.initialise();

    await workspace.saveRule(aRule("new", true), null);
    await workspace.refreshFolderStates();

    expect(backend.saved).toBe(0);
    expect(workspace.savesBlocked).toBe(true);
    expect(workspace.loadProblem).toBe(en["error.rulesUnreachable"]);
  });

  test("a malformed rule file can be replaced, since a copy was set aside", async () => {
    backend.failNext = { code: "rulesMalformed", message: "the rule file is not valid Tidyfile JSON" };
    const workspace = new Workspace();
    await workspace.initialise();

    await workspace.saveRule(aRule("new", true), null);

    expect(backend.saved).toBe(1);
    expect(workspace.savesBlocked).toBe(false);
  });

  test("a settings file that could not be read is never overwritten", async () => {
    backend.settingsUnreachable = true;
    const workspace = new Workspace();
    await workspace.initialise();

    await workspace.addFolder("/new");

    expect(backend.settingsSaved).toBe(0);
    expect(workspace.savesBlocked).toBe(true);
  });

  test("a fresh start shows what would change before anything can run", async () => {
    backend.rules = [aRule("a", true)];
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix", language: "auto" };
    backend.preview = [{ kind: "move", source: "/watched/a.pdf", destination: "/out/a.pdf" }];
    const workspace = new Workspace();

    await workspace.initialise();

    expect(workspace.preview).toHaveLength(1);
    expect(backend.simulated).toBe(1);
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
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix", language: "auto" };
    const workspace = new Workspace();
    await workspace.initialise();

    expect(workspace.canRun).toBe(false);
  });

  test("can run with a folder and an enabled rule", async () => {
    backend.rules = [aRule("a", true)];
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix", language: "auto" };
    const workspace = new Workspace();
    await workspace.initialise();

    expect(workspace.canRun).toBe(true);
  });

  test("tidying is not offered while a saved rule's new preview is on its way", async () => {
    backend.rules = [aRule("a", true)];
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix", language: "auto" };
    backend.preview = [{ kind: "move", source: "/a", destination: "/b" }];
    const workspace = new Workspace();
    await workspace.initialise();
    expect(workspace.canRun).toBe(true);
    backend.gate = () => undefined;

    const saving = workspace.saveRule({ ...aRule("a", true), name: "renamed" }, aRule("a", true));
    await Promise.resolve();
    await Promise.resolve();

    expect(workspace.preview).toBeNull();
    expect(workspace.canRun).toBe(false);
    backend.gate();
    await saving;
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
  test("saving a rule saves once and simulates once", async () => {
    backend.rules = [aRule("a", true)];
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix", language: "auto" };
    const workspace = new Workspace();
    await workspace.initialise();
    backend.saved = 0;
    backend.simulated = 0;

    await workspace.saveRule({ ...aRule("a", true), name: "renamed" }, aRule("a", true));

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

describe("editing rules", () => {
  test("a new rule is saved switched off and at the end", async () => {
    backend.rules = [aRule("a", true)];
    const workspace = new Workspace();
    await workspace.initialise();

    const outcome = await workspace.saveRule(aRule("new", true), null);

    expect(outcome).toBe("added");
    expect(backend.rules.map((rule) => [rule.id, rule.enabled])).toEqual([
      ["a", true],
      ["new", false],
    ]);
  });

  test("editing an active rule while watching switches it off and says so", async () => {
    const original = { ...aRule("a", true), name: "Facturas" };
    backend.rules = [original];
    backend.settings = { folders: ["/w"], watched: ["/w"], onCollision: "suffix", language: "auto" };
    const workspace = new Workspace();
    await workspace.initialise();

    const draft = { ...original, conditions: [{ type: "extension" as const, anyOf: ["jpg"] }] };
    const outcome = await workspace.saveRule(draft, original);

    expect(outcome).toBe("deactivated");
    expect(backend.rules[0]?.enabled).toBe(false);
    expect(workspace.deactivatedRule).toBe("Facturas");
  });

  test("renaming an active rule while watching keeps it on", async () => {
    const original = aRule("a", true);
    backend.rules = [original];
    backend.settings = { folders: ["/w"], watched: ["/w"], onCollision: "suffix", language: "auto" };
    const workspace = new Workspace();
    await workspace.initialise();

    const outcome = await workspace.saveRule({ ...original, name: "Otro nombre" }, original);

    expect(outcome).toBe("updated");
    expect(backend.rules[0]?.enabled).toBe(true);
    expect(workspace.deactivatedRule).toBeNull();
  });

  test("editing an active rule with nothing watched keeps it on", async () => {
    const original = aRule("a", true);
    backend.rules = [original];
    const workspace = new Workspace();
    await workspace.initialise();

    const draft = { ...original, combinator: "any" as const };
    const outcome = await workspace.saveRule(draft, original);

    expect(outcome).toBe("updated");
    expect(backend.rules[0]?.enabled).toBe(true);
  });

  test("the example looks only at the active rules before this one", async () => {
    backend.rules = [aRule("first", true), aRule("off", false), aRule("edited", true), aRule("after", true)];
    backend.settings = { folders: ["/w"], watched: [], onCollision: "suffix", language: "auto" };
    const workspace = new Workspace();
    await workspace.initialise();

    await workspace.exampleFor(aRule("edited", true));
    await workspace.exampleFor(aRule("brand-new", false));

    expect(backend.exampleCalls.map((call) => call.earlier.map((rule) => rule.id))).toEqual([
      ["first"],
      ["first", "edited", "after"],
    ]);
    expect(backend.exampleCalls[0]?.folders).toEqual(["/w"]);
  });

  test("choosing a language saves it and applies it", async () => {
    const workspace = new Workspace();
    await workspace.initialise();

    await workspace.setLanguage("es");

    expect(backend.settings.language).toBe("es");
    expect(i18n.locale).toBe("es");
  });

  test("an unreadable settings file still follows the system language", async () => {
    vi.stubGlobal("navigator", { language: "es-ES" });
    backend.settingsUnreachable = true;
    const workspace = new Workspace();

    await workspace.initialise();

    expect(i18n.locale).toBe("es");
  });

  test("a settings file without language starts as auto", async () => {
    backend.settings = { folders: [], watched: [], onCollision: "suffix" } as unknown as Settings;
    const workspace = new Workspace();

    await workspace.initialise();

    expect(workspace.language).toBe("auto");
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
    backend.settings = { folders: ["/one", "/two"], watched: [], onCollision: "suffix", language: "auto" };
    const workspace = new Workspace();
    await workspace.initialise();

    await workspace.removeFolder("/one");

    expect(workspace.folders).toEqual(["/two"]);
  });

  test("a folder that can never be used is refused when picked, and nothing is saved", async () => {
    backend.settings = { folders: ["/downloads"], watched: [], onCollision: "suffix", language: "auto" };
    backend.preview = [{ kind: "move", source: "/downloads/a.pdf", destination: "/out/a.pdf" }];
    backend.forbidden = ["C:\\Windows"];
    const workspace = new Workspace();
    await workspace.initialise();

    await workspace.addFolder("C:\\Windows");

    expect(workspace.folders).toEqual(["/downloads"]);
    expect(backend.settingsSaved).toBe(0);
    expect(workspace.refusedFolder).toBe("C:\\Windows");
    expect(workspace.preview).toEqual(backend.preview);
  });

  test("picking a usable folder afterwards clears the refusal", async () => {
    backend.forbidden = ["C:\\Windows"];
    const workspace = new Workspace();
    await workspace.initialise();
    await workspace.addFolder("C:\\Windows");

    await workspace.addFolder("/downloads");

    expect(workspace.folders).toEqual(["/downloads"]);
    expect(workspace.refusedFolder).toBeNull();
  });

  test("picking the same folder twice while it is being checked keeps one entry", async () => {
    const workspace = new Workspace();
    await workspace.initialise();

    await Promise.all([workspace.addFolder("/slow-nas"), workspace.addFolder("/slow-nas")]);

    expect(workspace.folders).toEqual(["/slow-nas"]);
  });

  test("a folder that could not be checked is not added", async () => {
    const workspace = new Workspace();
    await workspace.initialise();
    backend.statusFails = true;

    await workspace.addFolder("C:\\Windows");

    expect(workspace.folders).toEqual([]);
    expect(backend.settingsSaved).toBe(0);
    expect(workspace.status).toEqual({ kind: "problem", message: en["error.unavailable"] });
  });

  test("a folder saved twice by an older version is shown once", async () => {
    backend.settings = { folders: ["/one", "/one", "/two"], watched: [], onCollision: "suffix", language: "auto" };
    const workspace = new Workspace();

    await workspace.initialise();

    expect(workspace.folders).toEqual(["/one", "/two"]);
  });
});

describe("failures", () => {
  test("a backend error becomes a readable problem and clears the preview", async () => {
    backend.rules = [aRule("a", true)];
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix", language: "auto" };
    const workspace = new Workspace();
    await workspace.initialise();
    backend.failNext = { code: "forbiddenFolder", message: "this folder cannot be watched" };

    await workspace.refreshPreview();

    expect(workspace.status).toEqual({
      kind: "problem",
      message: en["error.forbiddenFolder"],
    });
    expect(workspace.preview).toBeNull();
  });

  test("an unrecognised failure still yields a message rather than crashing", async () => {
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix", language: "auto" };
    const workspace = new Workspace();
    await workspace.initialise();
    backend.failNext = { code: 42, message: undefined } as never;

    await workspace.refreshPreview();

    expect(workspace.status.kind).toBe("problem");
  });

  test("a later success clears the problem", async () => {
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix", language: "auto" };
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
    backend.settings = { folders: ["/one", "/two"], watched: [], onCollision: "suffix", language: "auto" };
    const workspace = new Workspace();
    await workspace.initialise();

    await workspace.setWatched("/one", true);

    expect(workspace.isWatched("/one")).toBe(true);
    expect(workspace.isWatched("/two")).toBe(false);
    expect(backend.settings.watched).toEqual(["/one"]);
  });

  test("unwatching the last folder stops watching altogether", async () => {
    backend.settings = { folders: ["/one"], watched: ["/one"], onCollision: "suffix", language: "auto" };
    const workspace = new Workspace();
    await workspace.initialise();

    await workspace.setWatched("/one", false);

    expect(workspace.watched).toEqual([]);
  });

  test("removing a folder also stops watching it", async () => {
    backend.settings = { folders: ["/one", "/two"], watched: ["/one"], onCollision: "suffix", language: "auto" };
    const workspace = new Workspace();
    await workspace.initialise();

    await workspace.removeFolder("/one");

    expect(workspace.folders).toEqual(["/two"]);
    expect(workspace.isWatched("/one")).toBe(false);
  });
});

describe("folder status", () => {
  test("checking again notices a folder that went away while the app was open", async () => {
    backend.settings = { folders: ["/usb"], watched: [], onCollision: "suffix", language: "auto" };
    const workspace = new Workspace();
    await workspace.initialise();
    expect(workspace.stateOf("/usb")).toBe("ok");

    backend.unreachable = ["/usb"];
    await workspace.refreshFolderStates();

    expect(workspace.stateOf("/usb")).toBe("unavailable");
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

describe("restoring from the trash", () => {
  test("an undo the system cannot do is reported, not swallowed", async () => {
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix", language: "auto" };
    backend.manualRestore = 2;
    const workspace = new Workspace();
    await workspace.initialise();

    await workspace.revert("b1");

    expect(workspace.manualRestore).toBe(2);
  });

  test("an undo that worked leaves no lingering notice", async () => {
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix", language: "auto" };
    backend.manualRestore = 0;
    const workspace = new Workspace();
    await workspace.initialise();

    await workspace.revert("b1");

    expect(workspace.manualRestore).toBe(0);
  });

  test("a file no longer in the trash is reported, not swallowed", async () => {
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix", language: "auto" };
    backend.notInTrash = 1;
    const workspace = new Workspace();
    await workspace.initialise();

    await workspace.revert("b1");

    expect(workspace.notInTrash).toBe(1);
  });

  test("a file that could not be put back is reported", async () => {
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix", language: "auto" };
    backend.undoFailed = 1;
    const workspace = new Workspace();
    await workspace.initialise();

    await workspace.revert("b1");

    expect(workspace.couldNotPutBack).toBe(1);
  });

  test("a later clean undo clears the earlier notices", async () => {
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix", language: "auto" };
    backend.notInTrash = 1;
    backend.undoFailed = 1;
    const workspace = new Workspace();
    await workspace.initialise();
    await workspace.revert("b1");

    backend.notInTrash = 0;
    backend.undoFailed = 0;
    await workspace.revert("b1");

    expect([workspace.notInTrash, workspace.couldNotPutBack]).toEqual([0, 0]);
  });

  test("a file moved or deleted since is reported, not swallowed", async () => {
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix", language: "auto" };
    backend.alreadyGone = 2;
    const workspace = new Workspace();
    await workspace.initialise();

    await workspace.revert("b1");

    expect(workspace.alreadyGone).toBe(2);
  });

  test("undoing a single file reports the same notices", async () => {
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix", language: "auto" };
    backend.notInTrash = 1;
    const workspace = new Workspace();
    await workspace.initialise();

    await workspace.revertOne(7);

    expect(workspace.notInTrash).toBe(1);
  });

  test("an undo that fails outright does not leave the previous notices behind", async () => {
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix", language: "auto" };
    backend.notInTrash = 1;
    backend.undoFailed = 1;
    backend.manualRestore = 1;
    backend.alreadyGone = 1;
    const workspace = new Workspace();
    await workspace.initialise();
    await workspace.revert("b1");

    backend.failNext = { code: "journal", message: "the history could not be reached" };
    await workspace.revert("b1");

    expect([
      workspace.manualRestore,
      workspace.notInTrash,
      workspace.couldNotPutBack,
      workspace.alreadyGone,
    ]).toEqual([0, 0, 0, 0]);
    expect(workspace.status.kind).toBe("problem");
  });
});

describe("what a run reports back", () => {
  test("the outcome is kept so the interface can show it", async () => {
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix", language: "auto" };
    backend.report = { batch: "b1", applied: 3, skipped: 0, failed: 0, needsManualRestore: 0, notInTrash: 0, alreadyGone: 0 };
    const workspace = new Workspace();
    await workspace.initialise();

    await workspace.run();

    expect(workspace.lastRun?.applied).toBe(3);
  });

  test("files left waiting for a decision are reported, not swallowed", async () => {
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "ask", language: "auto" };
    backend.report = { batch: "b1", applied: 0, skipped: 2, failed: 0, needsManualRestore: 0, notInTrash: 0, alreadyGone: 0 };
    const workspace = new Workspace();
    await workspace.initialise();

    await workspace.run();

    expect(workspace.lastRun?.skipped).toBe(2);
  });

  test("saving a rule clears a stale outcome", async () => {
    backend.rules = [aRule("a", true)];
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix", language: "auto" };
    const workspace = new Workspace();
    await workspace.initialise();
    await workspace.run();
    expect(workspace.lastRun).not.toBeNull();

    await workspace.saveRule({ ...aRule("a", true), name: "renamed" }, aRule("a", true));

    expect(workspace.lastRun).toBeNull();
  });
});

describe("the preview is what runs", () => {
  test("a folder that changed since the preview is shown again instead of tidied", async () => {
    backend.rules = [aRule("a", true)];
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix", language: "auto" };
    const first = { kind: "move" as const, source: "/watched/a.pdf", destination: "/out/a.pdf" };
    backend.preview = [first];
    const workspace = new Workspace();
    await workspace.initialise();

    backend.preview = [first, { kind: "move", source: "/watched/b.pdf", destination: "/out/b.pdf" }];
    await workspace.run();

    expect(backend.organized).toBe(0);
    expect(workspace.preview).toHaveLength(2);
    expect(workspace.previewChanged).toBe(true);
  });

  test("a second press runs what is now on screen", async () => {
    backend.rules = [aRule("a", true)];
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix", language: "auto" };
    const workspace = new Workspace();
    await workspace.initialise();
    backend.preview = [{ kind: "trash", source: "/watched/new.pdf", destination: null }];
    await workspace.run();

    await workspace.run();

    expect(backend.organized).toBe(1);
    expect(workspace.previewChanged).toBe(false);
  });
});

describe("two things happening at once", () => {
  test("the interface stays busy until the last one finishes", async () => {
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix", language: "auto" };
    const workspace = new Workspace();
    await workspace.initialise();

    backend.gate = () => undefined;
    const first = workspace.refreshPreview();
    const second = workspace.refreshPreview();
    expect(workspace.status.kind).toBe("working");

    backend.gate?.();
    backend.gate = null;
    await Promise.all([first, second]);

    expect(workspace.status.kind).toBe("idle");
  });

  test("a failure during overlapping work is not erased by the other finishing", async () => {
    backend.settings = { folders: ["/watched"], watched: [], onCollision: "suffix", language: "auto" };
    const workspace = new Workspace();
    await workspace.initialise();
    backend.failNext = { code: "folderNotFound", message: "gone" };

    await Promise.all([workspace.refreshPreview(), workspace.refreshPreview()]);

    expect(workspace.status.kind).toBe("problem");
  });
});
