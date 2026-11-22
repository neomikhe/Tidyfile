<script lang="ts">
  import type { Condition } from "../ipc";
  import { blankCondition, conditionLabels, conditionTypes, type ConditionType } from "../factories";
  import { t } from "../i18n/index.svelte";
  import Icon from "../ui/Icon.svelte";
  import ExtensionField from "./ExtensionField.svelte";
  import SizeField from "./SizeField.svelte";

  let {
    condition,
    problem,
    canRemove,
    onchange,
    onremove,
    onadd,
    onverify,
  }: {
    condition: Condition;
    problem: boolean;
    canRemove: boolean;
    onchange: (condition: Condition) => void;
    onremove: () => void;
    onadd: () => void;
    onverify: (kind: "glob" | "regex", pattern: string) => void;
  } = $props();
</script>

<div class="row">
  <select
    value={condition.type}
    aria-label={t("editor.conditionType")}
    onchange={(event) => onchange(blankCondition(event.currentTarget.value as ConditionType))}
  >
    {#each conditionTypes as type (type)}
      <option value={type}>{t(conditionLabels[type])}</option>
    {/each}
  </select>

  {#if condition.type === "extension"}
    <ExtensionField anyOf={condition.anyOf} onchange={(anyOf) => onchange({ type: "extension", anyOf })} />
  {:else if condition.type === "nameContains"}
    <input
      class="grow"
      placeholder={t("editor.placeholderContains")}
      aria-label={t("condition.nameContains")}
      value={condition.text}
      oninput={(event) => onchange({ type: "nameContains", text: event.currentTarget.value })}
    />
  {:else if condition.type === "nameMatchesGlob" || condition.type === "nameMatchesRegex"}
    {@const kind = condition.type === "nameMatchesGlob" ? "glob" : "regex"}
    <input
      class="grow"
      placeholder={t(kind === "glob" ? "editor.placeholderGlob" : "editor.placeholderRegex")}
      aria-label={t(conditionLabels[condition.type])}
      aria-invalid={problem}
      value={condition.pattern}
      oninput={(event) => onchange({ type: condition.type, pattern: event.currentTarget.value })}
      onchange={(event) => onverify(kind, event.currentTarget.value)}
    />
  {:else if condition.type === "largerThan" || condition.type === "smallerThan"}
    {@const type = condition.type}
    <SizeField bytes={condition.bytes} label={t(conditionLabels[type])} onchange={(bytes) => onchange({ type, bytes })} />
    <span class="grow"></span>
  {:else if condition.type === "olderThan" || condition.type === "newerThan"}
    <input
      type="number"
      min="0"
      class="amount"
      aria-label={t(conditionLabels[condition.type])}
      value={condition.days}
      onchange={(event) => onchange({ type: condition.type, days: Math.max(Math.round(Number(event.currentTarget.value)), 0) })}
    />
    <span class="grow muted">{t("editor.days")}</span>
  {:else if condition.type === "inSubfolder"}
    <input
      class="grow"
      placeholder={t("editor.placeholderSubfolder")}
      aria-label={t("condition.inSubfolder")}
      value={condition.name}
      oninput={(event) => onchange({ type: "inSubfolder", name: event.currentTarget.value })}
    />
  {/if}

  <span class="pm">
    <button class="round" aria-label={t("editor.removeCondition")} disabled={!canRemove} onclick={onremove}>
      <Icon name="minus" />
    </button>
    <button class="round" aria-label={t("editor.addCondition")} onclick={onadd}>
      <Icon name="plus" />
    </button>
  </span>
</div>
{#if problem}
  <p class="field-problem" role="alert">{t("editor.patternInvalid")}</p>
{/if}

<style>
  .amount {
    width: 80px;
  }

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

  .field-problem {
    margin: 0;
    padding: 0 12px 8px;
    color: var(--danger);
    font-size: 12px;
  }
</style>
