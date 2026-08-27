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
  resolveConflicts,
  folderStatus,
  watchedFolders,
  type ActivityEntry,
  type Collision,
  type PlannedChange,
  type BatchReport,
  type FolderStatus,
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
  folders = $state<string[]>([]);
  preview = $state<PlannedChange[] | null>(null);
  history = $state<ActivityEntry[]>([]);
  status = $state<Status>({ kind: "idle" });
  watched = $state<string[]>([]);
  unfinished = $state<PlannedChange[]>([]);
  onCollision = $state<Collision>("suffix");
  expanded = $state<string | null>(null);
  details = $state<RecordedChange[]>([]);
  folderStates = $state<FolderStatus[]>([]);
  manualRestore = $state(0);
  lastRun = $state<BatchReport | null>(null);
  private unlisten: UnlistenFn | null = null;

  get enabledRules(): Rule[] {
    return this.rules.filter((rule) => rule.enabled);
  }

  get canRun(): boolean {
    return (
      this.folders.length > 0 &&
      this.enabledRules.length > 0 &&
      this.status.kind !== "working"
    );
  }

  async initialise(): Promise<void> {
    await this.attempt(async () => {
      this.rules = await loadRules();
      this.history = await activity();
      this.unfinished = await interrupted();
      const settings = await loadSettings();
      this.folders = settings.folders;
      this.onCollision = settings.onCollision;
      this.watched = await watchedFolders();
      this.folderStates = await folderStatus(settings.folders);
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

  isWatched(folder: string): boolean {
    return this.watched.includes(folder);
  }

  async setWatched(folder: string, active: boolean): Promise<void> {
    const next = active
      ? [...this.watched, folder]
      : this.watched.filter((kept) => kept !== folder);
    await this.attempt(async () => {
      if (next.length === 0) {
        await stopWatching();
      } else {
        await startWatching(next);
      }
      this.watched = next;
    });
    await this.rememberSettings();
  }

  private async afterAutomaticTidy(): Promise<void> {
    await this.attempt(async () => {
      this.history = await activity();
      if (this.folders.length > 0) {
        this.preview = await simulate(this.enabledRules, this.folders);
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
    this.lastRun = null;
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

  async addFolder(picked: string): Promise<void> {
    if (this.folders.includes(picked)) {
      return;
    }
    this.folders = [...this.folders, picked];
    this.preview = null;
    await this.rememberSettings();
    await this.refreshFolderStates();
    await this.refreshPreview();
  }

  async removeFolder(folder: string): Promise<void> {
    this.folders = this.folders.filter((kept) => kept !== folder);
    if (this.isWatched(folder)) {
      await this.setWatched(folder, false);
    }
    this.preview = null;
    await this.rememberSettings();
    await this.refreshPreview();
  }

  async setCollision(policy: Collision): Promise<void> {
    this.onCollision = policy;
    await this.rememberSettings();
  }

  stateOf(folder: string): string {
    return this.folderStates.find((entry) => entry.folder === folder)?.state ?? "ok";
  }

  private async refreshFolderStates(): Promise<void> {
    const folders = this.folders;
    await this.attempt(async () => {
      this.folderStates = await folderStatus(folders);
    });
  }

  private async rememberSettings(): Promise<void> {
    const snapshot = {
      folders: this.folders,
      watched: this.watched,
      onCollision: this.onCollision,
    };
    await this.attempt(() => saveSettings(snapshot));
  }

  async refreshPreview(): Promise<void> {
    if (this.folders.length === 0) {
      return;
    }
    const folders = this.folders;
    await this.attempt(async () => {
      this.preview = await simulate(this.enabledRules, folders);
    });
  }

  async run(): Promise<void> {
    if (this.folders.length === 0) {
      return;
    }
    const folders = this.folders;
    this.lastRun = null;
    await this.attempt(async () => {
      this.lastRun = await organize(this.enabledRules, folders);
      this.history = await activity();
      this.preview = await simulate(this.enabledRules, folders);
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

  async decideConflicts(batch: string, keepBoth: boolean): Promise<void> {
    await this.attempt(async () => {
      await resolveConflicts(batch, keepBoth);
      this.history = await activity();
      if (this.expanded === batch) {
        this.details = await operationsIn(batch);
      }
      await this.refreshPreview();
    });
  }

  async revertOne(id: number): Promise<void> {
    await this.attempt(async () => {
      this.manualRestore = (await undoOperation(id)).needsManualRestore;
      this.history = await activity();
      if (this.expanded !== null) {
        this.details = await operationsIn(this.expanded);
      }
      if (this.folders.length > 0) {
        this.preview = await simulate(this.enabledRules, this.folders);
      }
    });
  }

  async revert(batch: string): Promise<void> {
    await this.attempt(async () => {
      this.manualRestore = (await undo(batch)).needsManualRestore;
      this.history = await activity();
      if (this.expanded !== null) {
        this.details = await operationsIn(this.expanded);
      }
      if (this.folders.length > 0) {
        this.preview = await simulate(this.enabledRules, this.folders);
      }
    });
  }

  private inFlight = 0;
  private failure: string | null = null;

  private async attempt(work: () => Promise<void>): Promise<void> {
    this.inFlight += 1;
    this.status = { kind: "working" };
    try {
      await work();
    } catch (error) {
      this.preview = null;
      this.failure = describe(error);
    } finally {
      this.inFlight -= 1;
      if (this.inFlight === 0) {
        this.status =
          this.failure === null ? { kind: "idle" } : { kind: "problem", message: this.failure };
        this.failure = null;
      }
    }
  }
}
