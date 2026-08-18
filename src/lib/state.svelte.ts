import {
  activity,
  isIpcError,
  loadRules,
  organize,
  saveRules,
  simulate,
  undo,
  type ActivityEntry,
  type PlannedChange,
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
    await this.refreshPreview();
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

  async revert(batch: string): Promise<void> {
    await this.attempt(async () => {
      await undo(batch);
      this.history = await activity();
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
