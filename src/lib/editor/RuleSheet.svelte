<script lang="ts">
  import { onMount } from "svelte";
  import { checkPattern, type Action, type Combinator, type Condition, type Rule, type RuleExample as Example } from "../ipc";
  import { blankAction, blankCondition, blankRule, combinators, isIncomplete } from "../factories";
  import { t } from "../i18n/index.svelte";
  import { detectPlatform } from "../platform";
  import type { Workspace } from "../state.svelte";
  import ConditionRow from "./ConditionRow.svelte";
  import ActionRow from "./ActionRow.svelte";
  import RuleExample from "./RuleExample.svelte";

  let { workspace, rule, onclose }: { workspace: Workspace; rule: Rule | null; onclose: () => void } = $props();

  let dialog = $state<HTMLDialogElement | null>(null);
  let draft = $state<Rule>(startingDraft());
  let problems = $state<Record<number, boolean>>({});
  let confirmingDelete = $state(false);
  let example = $state<Example | null>(null);
  const platform = detectPlatform(globalThis.navigator?.userAgent ?? "");
  const opener = typeof document === "undefined" ? null : document.activeElement;
  const blocked = $derived(isIncomplete(draft) || Object.values(problems).some(Boolean));

  function startingDraft(): Rule {
    return rule === null ? { ...blankRule(), name: t("editor.newTitle") } : $state.snapshot(rule);
  }

  onMount(() => dialog?.showModal());

  $effect(() => {
    const snapshot = $state.snapshot(draft);
    let current = true;
    const timer = setTimeout(async () => {
      const found = await workspace.exampleFor(snapshot);
      if (current) example = found;
    }, 400);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  });

  function closed(): void {
    if (opener instanceof HTMLElement) opener.focus();
    onclose();
  }

  function setConditions(conditions: Condition[]): void {
    draft.conditions = conditions;
  }

  function setActions(actions: Action[]): void {
    draft.actions = actions;
  }

  function insertAfter<T>(items: T[], index: number, item: T): T[] {
    return [...items.slice(0, index + 1), item, ...items.slice(index + 1)];
  }

  async function verify(index: number, kind: "glob" | "regex", pattern: string): Promise<void> {
    try {
      if (pattern.trim().length > 0) await checkPattern(kind, pattern);
      problems = { ...problems, [index]: false };
    } catch {
      problems = { ...problems, [index]: true };
    }
  }

  async function save(): Promise<void> {
    await workspace.saveRule($state.snapshot(draft), rule === null ? null : $state.snapshot(rule));
    dialog?.close();
  }

  async function remove(): Promise<void> {
    if (rule === null) return;
    await workspace.remove(rule.id);
    dialog?.close();
  }
</script>

<dialog bind:this={dialog} class="sheet" aria-labelledby="sheet-title" onclose={closed}>
  <h2 id="sheet-title">{t(rule === null ? "editor.newTitle" : "editor.editTitle")}</h2>

  <label class="line">
    <span class="lab">{t("editor.name")}</span>
    <input class="grow" bind:value={draft.name} />
  </label>

  <div class="line section">
    <span>{t("editor.matchPrefix")}</span>
    <select aria-label={t("editor.combinator")} bind:value={draft.combinator}>
      {#each combinators as combinator (combinator)}
        <option value={combinator as Combinator}>{t(combinator === "all" ? "editor.all" : "editor.any")}</option>
      {/each}
    </select>
    <span>{t("editor.matchSuffix")}</span>
  </div>
  <div class="box">
    {#each draft.conditions as condition, index (index)}
      <ConditionRow
        {condition}
        problem={problems[index] ?? false}
        canRemove={draft.conditions.length > 1}
        onchange={(changed) => setConditions(draft.conditions.map((item, at) => (at === index ? changed : item)))}
        onremove={() => setConditions(draft.conditions.filter((_, at) => at !== index))}
        onadd={() => setConditions(insertAfter(draft.conditions, index, blankCondition("extension")))}
        onverify={(kind, pattern) => verify(index, kind, pattern)}
      />
    {/each}
  </div>

  <p class="section">{t("editor.then")}</p>
  <div class="box">
    {#each draft.actions as action, index (index)}
      <ActionRow
        {action}
        {platform}
        canRemove={draft.actions.length > 1}
        onchange={(changed) => setActions(draft.actions.map((item, at) => (at === index ? changed : item)))}
        onremove={() => setActions(draft.actions.filter((_, at) => at !== index))}
        onadd={() => setActions(insertAfter(draft.actions, index, blankAction("moveTo")))}
      />
    {/each}
  </div>

  {#if example !== null}
    <RuleExample {example} />
  {/if}

  <div class="footer">
    {#if rule !== null}
      {#if confirmingDelete}
        <span class="confirm">{t("editor.deleteConfirm", { name: rule.name })}</span>
        <button class="danger" onclick={remove}>{t("editor.deleteYes")}</button>
        <button onclick={() => (confirmingDelete = false)}>{t("editor.deleteNo")}</button>
      {:else}
        <button class="danger" onclick={() => (confirmingDelete = true)}>{t("editor.delete")}</button>
      {/if}
    {/if}
    <span class="grow"></span>
    {#if blocked}
      <span class="muted">{t("editor.incompleteHint")}</span>
    {/if}
    <button onclick={() => dialog?.close()}>{t("editor.cancel")}</button>
    <button class="primary" disabled={blocked} onclick={save}>{t("editor.save")}</button>
  </div>
</dialog>

<style>
  .sheet {
    width: min(640px, calc(100vw - 32px));
    max-height: calc(100vh - 80px);
    margin: 52px auto auto;
    padding: 18px 20px 16px;
    border: 0;
    border-radius: 0 0 12px 12px;
    background: var(--sheet-bg);
    color: var(--text);
    box-shadow: var(--shadow-sheet);
    overflow: auto;
    animation: slide 150ms ease-out;
  }

  .sheet::backdrop {
    background: var(--scrim);
  }

  @keyframes slide {
    from {
      transform: translateY(-16px);
      opacity: 0;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .sheet {
      animation: none;
    }
  }

  h2 {
    margin: 0 0 12px;
    font-size: 15px;
  }

  .line {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .lab {
    width: 64px;
    text-align: right;
    color: var(--text-secondary);
  }

  .grow {
    flex: 1;
    min-width: 0;
  }

  .section {
    margin: 14px 0 6px;
    font-weight: 600;
  }

  .box {
    background: var(--group-bg);
    border: 1px solid var(--group-border);
    border-radius: var(--radius-group);
  }

  .footer {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 14px;
    flex-wrap: wrap;
  }

  .confirm {
    font-weight: 600;
  }
</style>
