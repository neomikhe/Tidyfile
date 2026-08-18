import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  activity,
  interrupted,
  isIpcError,
  loadRules,
  loadSettings,
  organize,
  saveRules,
  saveSettings,
  settleInterrupted,
  simulate,
  startWatching,
  stopWatching,
  undo,
  undoOperation,
  operations as operationsIn,
  watchedFolder,
  type ActivityEntry,
  type Collision,
  type PlannedChange,
  type RecordedChange,
  type Rule,
} from "./ipc";

export type Status = { kind: "idle" } | { kind: "working" } | { kind: "problem"; message: string };

function flip(rule: Rule): Rule {
  return { ...rule, enabled: !rule.enabled };
}

function describe(error: unknown): string {
  if (isIpcError(error)) {
    return error.message;
  }
  return "Something went wrong. Please try again.";
}

export class Workspace {
  rules = $state<Rule[]>([]);
  folder = $state<string | null>(null);
  preview = $state<PlannedChange[] | null>(null);
  history = $state<ActivityEntry[]>([]);
  status = $state<Status>({ kind: "idle" });
  watching = $state(false);
  unfinished = $state<PlannedChange[]>([]);
  onCollision = $state<Collision>("suffix");
  expanded = $state<string | null>(null);
  details = $state<RecordedChange[]>([]);
  private unlisten: UnlistenFn | null = null;

  get enabledRules(): Rule[] {
    return this.rules.filter((rule) => rule.enabled);
  }

  get canRun(): boolean {
    return this.folder !== null && this.enabledRules.length > 0 && this.status.kind !== "working";
  }

  async initialise(): Promise<void> {
    await this.attempt(async () => {
      this.rules = await loadRules();
      this.history = await activity();
      this.unfinished = await interrupted();
      const settings = await loadSettings();
      this.folder = settings.folder;
      this.onCollision = settings.onCollision;
      this.watching = (await watchedFolder()) !== null;
    });
    this.unlisten = await listen("tidied", () => {
      void this.afterAutomaticTidy();
    });
  }

  async acknowledgeUnfinished(): Promise<void> {
    await this.attempt(async () => {
      await settleInterrupted();
      this.unfinished = await interrupted();
      this.history = await activity();
    });
  }

  dispose(): void {
    this.unlisten?.();
    this.unlisten = null;
  }

  async setWatching(active: boolean): Promise<void> {
    if (active && this.folder === null) {
      return;
    }
    const folder = this.folder;
    await this.attempt(async () => {
      if (active && folder !== null) {
        await startWatching(folder);
      } else {
        await stopWatching();
      }
      this.watching = active;
    });
    await this.rememberSettings();
  }

  private async afterAutomaticTidy(): Promise<void> {
    await this.attempt(async () => {
      this.history = await activity();
      if (this.folder !== null) {
        this.preview = await simulate(this.enabledRules, this.folder);
      }
    });
  }

  async toggle(id: string): Promise<void> {
    await this.replace(this.rules.map((rule) => (rule.id === id ? flip(rule) : rule)));
  }

  async add(rule: Rule): Promise<void> {
    await this.replace([...this.rules, rule]);
  }

  edit(edited: Rule): void {
    this.rules = this.rules.map((rule) => (rule.id === edited.id ? edited : rule));
    this.preview = null;
  }

  async commit(): Promise<void> {
    await this.attempt(() => saveRules(this.rules));
    await this.refreshPreview();
  }

  async remove(id: string): Promise<void> {
    await this.replace(this.rules.filter((rule) => rule.id !== id));
  }

  private async replace(rules: Rule[]): Promise<void> {
    this.rules = rules;
    await this.attempt(() => saveRules(rules));
    await this.refreshPreview();
  }

  async chooseFolder(picked: string): Promise<void> {
    this.folder = picked;
    this.preview = null;
    await this.rememberSettings();
    await this.refreshPreview();
  }

  async setCollision(policy: Collision): Promise<void> {
    this.onCollision = policy;
    await this.rememberSettings();
  }

  private async rememberSettings(): Promise<void> {
    const snapshot = {
      folder: this.folder,
      watching: this.watching,
      onCollision: this.onCollision,
    };
    await this.attempt(() => saveSettings(snapshot));
  }

  async refreshPreview(): Promise<void> {
    if (this.folder === null) {
      return;
    }
    const folder = this.folder;
    await this.attempt(async () => {
      this.preview = await simulate(this.enabledRules, folder);
    });
  }

  async run(): Promise<void> {
    if (this.folder === null) {
      return;
    }
    const folder = this.folder;
    await this.attempt(async () => {
      await organize(this.enabledRules, folder);
      this.history = await activity();
      this.preview = await simulate(this.enabledRules, folder);
    });
  }

  async expand(batch: string): Promise<void> {
    if (this.expanded === batch) {
      this.expanded = null;
      this.details = [];
      return;
    }
    this.expanded = batch;
    await this.attempt(async () => {
      this.details = await operationsIn(batch);
    });
  }

  async revertOne(id: number): Promise<void> {
    await this.attempt(async () => {
      await undoOperation(id);
      this.history = await activity();
      if (this.expanded !== null) {
        this.details = await operationsIn(this.expanded);
      }
      if (this.folder !== null) {
        this.preview = await simulate(this.enabledRules, this.folder);
      }
    });
  }

  async revert(batch: string): Promise<void> {
    await this.attempt(async () => {
      await undo(batch);
      this.history = await activity();
      if (this.expanded !== null) {
        this.details = await operationsIn(this.expanded);
      }
      if (this.folder !== null) {
        this.preview = await simulate(this.enabledRules, this.folder);
      }
    });
  }

  private async attempt(work: () => Promise<void>): Promise<void> {
    this.status = { kind: "working" };
    try {
      await work();
      this.status = { kind: "idle" };
    } catch (error) {
      this.preview = null;
      this.status = { kind: "problem", message: describe(error) };
    }
  }
}
