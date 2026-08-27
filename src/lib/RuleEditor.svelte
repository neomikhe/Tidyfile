<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import { checkPattern, type Action, type Condition, type Rule } from "./ipc";
  import {
    actionLabels,
    actionTypes,
    blankAction,
    blankCondition,
    combinators,
    conditionLabels,
    conditionTypes,
    extensionsToText,
    textToExtensions,
    type ActionType,
    type ConditionType,
  } from "./factories";

  let {
    rule,
    onchange,
    onremove,
    ondone,
  }: {
    rule: Rule;
    onchange: (rule: Rule) => void;
    onremove: () => void;
    ondone: () => void;
  } = $props();

  let patternProblem = $state<Record<number, string>>({});

  async function verify(index: number, kind: "glob" | "regex", pattern: string): Promise<void> {
    if (pattern.trim().length === 0) {
      patternProblem = { ...patternProblem, [index]: "" };
      return;
    }
    try {
      await checkPattern(kind, pattern);
      patternProblem = { ...patternProblem, [index]: "" };
    } catch (error) {
      const message = typeof error === "object" && error !== null && "message" in error
        ? String((error as { message: unknown }).message)
        : "This pattern cannot be used.";
      patternProblem = { ...patternProblem, [index]: message };
    }
  }

  function edit(changes: Partial<Rule>): void {
    onchange({ ...rule, ...changes });
  }

  function replaceCondition(index: number, condition: Condition): void {
    edit({ conditions: rule.conditions.map((item, at) => (at === index ? condition : item)) });
  }

  function replaceAction(index: number, action: Action): void {
    edit({ actions: rule.actions.map((item, at) => (at === index ? action : item)) });
  }

  function dropCondition(index: number): void {
    edit({ conditions: rule.conditions.filter((_, at) => at !== index) });
  }

  function dropAction(index: number): void {
    edit({ actions: rule.actions.filter((_, at) => at !== index) });
  }

  async function chooseFolder(index: number, action: Action): Promise<void> {
    if (action.type !== "moveTo" && action.type !== "copyTo") {
      return;
    }
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") {
      replaceAction(index, { ...action, folder: picked });
    }
  }
</script>

<div class="editor">
  <label class="field">
    <span>Name</span>
    <input value={rule.name} oninput={(event) => edit({ name: event.currentTarget.value })} />
  </label>

  <label class="field">
    <span>Match</span>
    <select
      value={rule.combinator}
      onchange={(event) =>
        edit({ combinator: event.currentTarget.value as Rule["combinator"] })}
    >
      {#each combinators as option (option.value)}
        <option value={option.value}>{option.label}</option>
      {/each}
    </select>
  </label>

  <h4>Conditions</h4>
  {#each rule.conditions as condition, index (index)}
    <div class="row">
      <select
        value={condition.type}
        onchange={(event) =>
          replaceCondition(index, blankCondition(event.currentTarget.value as ConditionType))}
      >
        {#each conditionTypes as type (type)}
          <option value={type}>{conditionLabels[type]}</option>
        {/each}
      </select>

      {#if condition.type === "extension"}
        <input
          placeholder="pdf, jpg"
          value={extensionsToText(condition.anyOf)}
          oninput={(event) =>
            replaceCondition(index, {
              type: "extension",
              anyOf: textToExtensions(event.currentTarget.value),
            })}
        />
      {:else if condition.type === "nameContains"}
        <input
          placeholder="invoice"
          value={condition.text}
          oninput={(event) =>
            replaceCondition(index, { type: "nameContains", text: event.currentTarget.value })}
        />
      {:else if condition.type === "nameMatchesGlob" || condition.type === "nameMatchesRegex"}
        <input
          placeholder={condition.type === "nameMatchesGlob" ? "Screenshot*.png" : "^IMG_\\d+"}
          value={condition.pattern}
          aria-invalid={Boolean(patternProblem[index])}
          oninput={(event) =>
            replaceCondition(index, {
              type: condition.type,
              pattern: event.currentTarget.value,
            })}
          onchange={(event) =>
            verify(
              index,
              condition.type === "nameMatchesGlob" ? "glob" : "regex",
              event.currentTarget.value,
            )}
        />
        {#if patternProblem[index]}
          <span class="pattern-problem" role="alert">{patternProblem[index]}</span>
        {/if}
      {:else if condition.type === "largerThan" || condition.type === "smallerThan"}
        <input
          type="number"
          min="0"
          value={condition.bytes}
          oninput={(event) =>
            replaceCondition(index, {
              type: condition.type,
              bytes: Number(event.currentTarget.value),
            })}
        />
        <span class="unit">bytes</span>
      {:else if condition.type === "olderThan" || condition.type === "newerThan"}
        <input
          type="number"
          min="0"
          value={condition.days}
          oninput={(event) =>
            replaceCondition(index, {
              type: condition.type,
              days: Number(event.currentTarget.value),
            })}
        />
        <span class="unit">days</span>
      {:else if condition.type === "inSubfolder"}
        <input
          placeholder="Invoices"
          value={condition.name}
          oninput={(event) =>
            replaceCondition(index, { type: "inSubfolder", name: event.currentTarget.value })}
        />
      {/if}

      <button
        class="drop"
        aria-label="Remove condition"
        onclick={() => dropCondition(index)}
      >
        &times;
      </button>
    </div>
  {/each}
  <button onclick={() => edit({ conditions: [...rule.conditions, blankCondition("extension")] })}>
    Add condition
  </button>

  <h4>Actions</h4>
  {#each rule.actions as action, index (index)}
    <div class="row">
      <select
        value={action.type}
        onchange={(event) =>
          replaceAction(index, blankAction(event.currentTarget.value as ActionType))}
      >
        {#each actionTypes as type (type)}
          <option value={type}>{actionLabels[type]}</option>
        {/each}
      </select>

      {#if action.type === "moveTo" || action.type === "copyTo"}
        <span class="folder">{action.folder || "No folder chosen"}</span>
        <button onclick={() => chooseFolder(index, action)}>Choose</button>
        <input
          placeholder="Subfolder, e.g. {'{year}/{month}'}"
          value={action.subfolder ?? ""}
          oninput={(event) =>
            replaceAction(index, {
              ...action,
              subfolder: event.currentTarget.value || undefined,
            })}
        />
        <input
          placeholder="Rename, e.g. {'{date} {name}.{ext}'}"
          value={action.rename ?? ""}
          oninput={(event) =>
            replaceAction(index, { ...action, rename: event.currentTarget.value || undefined })}
        />
      {:else if action.type === "renameTo"}
        <input
          value={action.template}
          oninput={(event) =>
            replaceAction(index, { type: "renameTo", template: event.currentTarget.value })}
        />
      {/if}

      <button class="drop" aria-label="Remove action" onclick={() => dropAction(index)}>
        &times;
      </button>
    </div>
  {/each}
  <button onclick={() => edit({ actions: [...rule.actions, blankAction("moveTo")] })}>
    Add action
  </button>

  <p class="hint">
    Placeholders: {"{name}"} {"{ext}"} {"{date}"} {"{year}"} {"{month}"} {"{day}"} {"{counter}"}
  </p>

  <div class="footer">
    <button class="primary" onclick={ondone}>Done</button>
    <button class="danger" onclick={onremove}>Delete rule</button>
  </div>
</div>

<style>
  .editor {
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
    padding: 0.9rem;
    border: 1px solid;
    border-radius: 0.375rem;
    margin-block: 0.5rem 1rem;
  }

  h4 {
    margin-block: 0.75rem 0;
    font-size: 0.85rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    opacity: 0.75;
  }

  .field {
    display: flex;
    align-items: center;
    gap: 0.6rem;
  }

  .field > span {
    min-width: 4rem;
    font-size: 0.9rem;
  }

  .row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.4rem;
  }

  input,
  select {
    padding: 0.35rem 0.5rem;
    border: 1px solid;
    border-radius: 0.25rem;
    background: none;
    color: inherit;
    font: inherit;
    font-size: 0.9rem;
    min-width: 0;
    flex: 1 1 16rem;
  }

  select {
    flex: 0 0 auto;
  }

  button {
    padding: 0.35rem 0.7rem;
    border: 1px solid currentColor;
    border-radius: 0.25rem;
    background: none;
    color: inherit;
    font: inherit;
    font-size: 0.9rem;
    cursor: pointer;
    align-self: flex-start;
  }

  button.primary {
    background: CanvasText;
    color: Canvas;
  }

  button.drop {
    flex: 0 0 auto;
    padding: 0.2rem 0.5rem;
  }

  .folder,
  .unit {
    font-size: 0.85rem;
    opacity: 0.75;
  }

  .folder {
    font-family: ui-monospace, monospace;
    overflow-wrap: anywhere;
  }

  .pattern-problem {
    flex: 1 1 100%;
    font-size: 0.85rem;
    padding: 0.2rem 0.4rem;
    border: 1px solid;
    border-radius: 0.25rem;
  }

  .hint {
    font-size: 0.8rem;
    opacity: 0.7;
    font-family: ui-monospace, monospace;
    margin-block: 0.5rem 0;
  }

  .footer {
    display: flex;
    gap: 0.5rem;
    margin-top: 0.75rem;
  }
</style>
