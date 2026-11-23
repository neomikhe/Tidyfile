<script lang="ts">
  import { onMount } from "svelte";
  import { checkPattern, type Action, type Combinator, type Condition, type Rule } from "../ipc";
  import { blankAction, blankCondition, blankRule, combinators, isIncomplete } from "../factories";
  import { t } from "../i18n/index.svelte";
  import { detectPlatform } from "../platform";
  import type { ExampleResult, Workspace } from "../state.svelte";
  import ConditionRow from "./ConditionRow.svelte";
  import ActionRow from "./ActionRow.svelte";
  import RuleExample from "./RuleExample.svelte";
  import { addedAfter, hasProblem, keyed, replaced, valuesOf, without, type Keyed } from "./rows";

  let { workspace, rule, onclose }: { workspace: Workspace; rule: Rule | null; onclose: () => void } = $props();

  let dialog = $state<HTMLDialogElement | null>(null);
  const initial = startingDraft();
  let draft = $state<Rule>(initial);
  let conditionRows = $state(keyed(initial.conditions));
  let actionRows = $state(keyed(initial.actions));
  let problems = $state<Record<number, boolean>>({});
  let confirmingDelete = $state(false);
  let example = $state<ExampleResult>(null);
  const platform = detectPlatform(globalThis.navigator?.userAgent ?? "");
  const opener = typeof document === "undefined" ? null : document.activeElement;
  const blocked = $derived(isIncomplete(draft) || hasProblem(conditionRows, problems) || workspace.savesBlocked);

  function startingDraft(): Rule {
    return rule === null ? { ...blankRule(), name: t("editor.newTitle") } : $state.snapshot(rule);
  }

  onMount(() => dialog?.showModal());

  $effect(() => {
    const snapshot = $state.snapshot(draft);
    const history = workspace.history;
    let current = true;
    const timer = setTimeout(async () => {
      const found = await workspace.exampleFor(snapshot);
      if (current && history === workspace.history) example = found;
    }, 400);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  });

  function closed(): void {
    const back = opener instanceof HTMLElement && opener.isConnected ? opener : null;
    (back ?? document.querySelector<HTMLElement>("[data-new-rule]"))?.focus();
    onclose();
  }

  function setConditions(rows: Keyed<Condition>[]): void {
    conditionRows = rows;
    draft.conditions = valuesOf(rows);
  }

  function setActions(rows: Keyed<Action>[]): void {
    actionRows = rows;
    draft.actions = valuesOf(rows);
  }

  function changeCondition(row: Keyed<Condition>, changed: Condition): void {
    if (changed.type !== row.value.type) problems = { ...problems, [row.key]: false };
    setConditions(replaced(conditionRows, row.key, changed));
  }

  async function verify(key: number, kind: "glob" | "regex", pattern: string): Promise<void> {
    try {
      if (pattern.trim().length > 0) await checkPattern(kind, pattern);
      problems = { ...problems, [key]: false };
    } catch {
      problems = { ...problems, [key]: true };
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
    {#each conditionRows as row (row.key)}
      <ConditionRow
        condition={row.value}
        problem={problems[row.key] ?? false}
        canRemove={conditionRows.length > 1}
        onchange={(changed) => changeCondition(row, changed)}
        onremove={() => setConditions(without(conditionRows, row.key))}
        onadd={() => setConditions(addedAfter(conditionRows, row.key, blankCondition("extension")))}
        onverify={(kind, pattern) => verify(row.key, kind, pattern)}
      />
    {/each}
  </div>

  <p class="section">{t("editor.then")}</p>
  <div class="box">
    {#each actionRows as row (row.key)}
      <ActionRow
        action={row.value}
        {platform}
        canRemove={actionRows.length > 1}
        onchange={(changed) => setActions(replaced(actionRows, row.key, changed))}
        onremove={() => setActions(without(actionRows, row.key))}
        onadd={() => setActions(addedAfter(actionRows, row.key, blankAction("moveTo")))}
      />
    {/each}
  </div>

  {#if example === "failed"}
    <p class="muted example-failed" role="status">{t("example.failed")}</p>
  {:else if example !== null}
    <RuleExample {example} />
  {/if}

  <div class="footer">
    {#if rule !== null}
      {#if confirmingDelete}
        <span class="confirm">{t("editor.deleteConfirm", { name: rule.name })}</span>
        <button class="danger" disabled={workspace.savesBlocked} onclick={remove}>{t("editor.deleteYes")}</button>
        <button onclick={() => (confirmingDelete = false)}>{t("editor.deleteNo")}</button>
      {:else}
        <button class="danger" disabled={workspace.savesBlocked} onclick={() => (confirmingDelete = true)}>
          {t("editor.delete")}
        </button>
      {/if}
    {/if}
    <span class="grow"></span>
    {#if workspace.savesBlocked}
      <span class="muted">{t("editor.savesBlocked")}</span>
    {:else if blocked}
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

  .example-failed {
    margin: 12px 0 0;
  }
</style>
