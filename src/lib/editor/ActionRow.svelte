<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import type { Action } from "../ipc";
  import { actionLabels, actionTypes, blankAction, type ActionType } from "../factories";
  import { knownFolderLabel, showable } from "../present";
  import { t } from "../i18n/index.svelte";
  import type { Platform } from "../platform";
  import Icon from "../ui/Icon.svelte";
  import InsertChips from "./InsertChips.svelte";

  let {
    action,
    canRemove,
    platform,
    onchange,
    onremove,
    onadd,
  }: {
    action: Action;
    canRemove: boolean;
    platform: Platform;
    onchange: (action: Action) => void;
    onremove: () => void;
    onadd: () => void;
  } = $props();

  type Field = "subfolder" | "rename" | "template";
  let lastField = $state<HTMLInputElement | null>(null);
  let defaultField = $state<HTMLInputElement | null>(null);

  async function chooseFolder(): Promise<void> {
    if (action.type !== "moveTo" && action.type !== "copyTo") {
      return;
    }
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") {
      onchange({ ...action, folder: picked });
    }
  }

  function write(field: Field, value: string): void {
    if (field === "template" && action.type === "renameTo") {
      onchange({ type: "renameTo", template: value });
    } else if (action.type === "moveTo" || action.type === "copyTo") {
      onchange({ ...action, [field]: value.length > 0 ? value : undefined });
    }
  }

  function target(): HTMLInputElement | null {
    return lastField?.isConnected ? lastField : defaultField;
  }

  function insert(token: string): void {
    const field = target();
    if (field === null) {
      return;
    }
    field.setRangeText(token, field.selectionStart ?? field.value.length, field.selectionEnd ?? field.value.length, "end");
    write(field.dataset["field"] as Field, field.value);
    field.focus();
  }
</script>

<div class="row">
  <select
    value={action.type}
    aria-label={t("editor.actionType")}
    onchange={(event) => onchange(blankAction(event.currentTarget.value as ActionType))}
  >
    {#each actionTypes as type (type)}
      <option value={type}>{t(actionLabels[type])}</option>
    {/each}
  </select>

  {#if action.type === "moveTo" || action.type === "copyTo"}
    <Icon name="folder" />
    <bdi class="grow ellipsis" title={showable(action.folder)}>
      {action.folder.length > 0 ? knownFolderLabel(action.folder) : t("editor.noFolder")}
    </bdi>
    <button onclick={chooseFolder}>{t("editor.choose")}</button>
  {:else if action.type === "renameTo"}
    <input
      class="grow"
      data-field="template"
      bind:this={defaultField}
      aria-label={t("editor.template")}
      value={action.template}
      onfocus={(event) => (lastField = event.currentTarget)}
      oninput={(event) => write("template", event.currentTarget.value)}
    />
  {:else}
    <span class="grow muted">{t(platform === "macos" ? "editor.trashNoteMac" : "editor.trashNote")}</span>
  {/if}

  <span class="pm">
    <button class="round" aria-label={t("editor.removeAction")} disabled={!canRemove} onclick={onremove}>
      <Icon name="minus" />
    </button>
    <button class="round" aria-label={t("editor.addAction")} onclick={onadd}>
      <Icon name="plus" />
    </button>
  </span>
</div>

{#if action.type === "moveTo" || action.type === "copyTo"}
  <label class="row sub">
    <span class="lab">{t("editor.subfolder")}</span>
    <input
      class="grow"
      data-field="subfolder"
      bind:this={defaultField}
      placeholder={t("editor.subfolderPlaceholder")}
      value={action.subfolder ?? ""}
      onfocus={(event) => (lastField = event.currentTarget)}
      oninput={(event) => write("subfolder", event.currentTarget.value)}
    />
  </label>
  <label class="row sub">
    <span class="lab">{t("editor.rename")}</span>
    <input
      class="grow"
      data-field="rename"
      placeholder={t("editor.renameKeep")}
      value={action.rename ?? ""}
      onfocus={(event) => (lastField = event.currentTarget)}
      oninput={(event) => write("rename", event.currentTarget.value)}
    />
  </label>
{/if}

{#if action.type !== "trash"}
  <InsertChips enabled={target() !== null} oninsert={insert} />
{/if}

<style>
  .pm {
    display: flex;
    gap: 4px;
  }

  .round {
    width: 24px;
    height: 24px;
    padding: 0;
    border-radius: 50%;
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }

  .sub {
    padding-left: 22px;
  }

  .lab {
    width: 86px;
    flex-shrink: 0;
    color: var(--text-secondary);
  }

  .row :global(svg) {
    color: var(--accent-text);
  }
</style>
